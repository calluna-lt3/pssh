use std::collections::{HashMap, VecDeque};
use std::env;
use std::fs::{self, read_dir, DirEntry};
use std::path::{Path, PathBuf};
use std::process::exit;
use std::time::SystemTime;

use chrono::{DateTime, Local};
use notify::{Event, EventKind, RecursiveMode, Watcher};
use tokio::{select, signal};

const DEFAULT_INBOX: &'static str = "./INBOX/";

struct Args {
    contents: Vec<String>,
    len: usize,
}

impl Args {
    fn new() -> Self {
        let args: Vec<String> = env::args().collect();
        let count = args.len();

        Self {
            contents: args,
            len: count,
        }
    }
}

impl Iterator for Args {
    type Item = String;
    fn next(&mut self) -> Option<Self::Item> {
        if self.len > 0 {
            self.len -= 1;
            Some(self.contents.remove(0))
        } else {
            None
        }
    }
}

fn usage() {
    println!("Usage: fm [OPTION] [ARGUMENT]");
}

fn init_inbox() -> String {
    let mut args = Args::new().into_iter();
    args.next(); // strip program name

    let directory = match args.next() {
        Some(opt) if opt == "-d" => {
            if let Some(dir) = args.next() {
                dir
            } else {
                usage();
                exit(1);
            }
        }
        Some(_) => {
            usage();
            exit(1);
        }
        None => String::from(DEFAULT_INBOX),
    };

    if let Ok(false) = fs::exists(&directory) {
        fs::create_dir(&directory).unwrap();
    }

    return directory;
}

fn find_files_in(path: &Path) -> Option<Vec<String>> {
    let start_files = read_dir(&path).unwrap();

    let mut cur_files: VecDeque<DirEntry> = start_files
        .map(|x| x.unwrap())
        .collect::<VecDeque<DirEntry>>();
    let mut ret_files: Vec<String> = vec![];

    loop {
        let file = match cur_files.pop_front() {
            Some(file) => file,
            None => break,
        };

        let file_md = file.metadata().unwrap();
        if file_md.is_dir() {
            let dir = read_dir(file.path()).unwrap();
            dir.for_each(|sub_file| cur_files.push_back(sub_file.unwrap()));
        } else {
            ret_files.push(file.path().to_string_lossy().to_string());
        }
    }

    if ret_files.len() > 0 {
        Some(ret_files)
    } else {
        None
    }
}

fn _print_index(index: &HashMap<String, SystemTime>) {
    index.into_iter().for_each(|(path, st)| {
        let dt = || -> String {
            let time: DateTime<Local> = DateTime::from(*st);
            time.format("%Y-%m-%d %H:%M").to_string()
        }();
        println!("[{dt}] {path}");
    });
}

fn _print_event(event: &Event) {
    match event.kind {
        EventKind::Create(_) => {
            event.paths.iter().for_each(|path| {
                print!("[NEW] ");
                println!("{file}", file = path.display());
            });
        }
        EventKind::Modify(_) => {
            event.paths.iter().for_each(|path| {
                print!("[MOD] ");
                println!("{file}", file = path.display());
            });
        }
        EventKind::Remove(_) => {
            event.paths.iter().for_each(|path| {
                print!("[DEL] ");
                println!("{file}", file = path.display());
            });
        }
        _ => return,
    };
}

struct FileIndex {
    index: HashMap<PathBuf, SystemTime>,
    location: PathBuf,
}

impl FileIndex {
    fn new(directory: PathBuf) -> Self {
        let files: Vec<String> = match find_files_in(&directory) {
            Some(x) => x,
            None => vec![],
        };

        let mut index: HashMap<PathBuf, SystemTime> = HashMap::new();

        for file in files {
            let md = fs::metadata(&file).unwrap();
            let time = md.modified().unwrap();
            index.insert(PathBuf::from(&file), time);
        }

        Self {
            index,
            location: directory,
        }
    }

    // Notify docs specify that there can be more than one file per event, however I haven't
    // observed this. This currently only handles the first file per event, and will display number
    // of events if > 1 event.
    fn handle_event(&mut self, event: &Event) {
        let k = event.paths[0].to_str().expect("path is not valid unicode");
        let i = k
            .find(self.location.to_str().expect("path is not valid unicode"))
            .unwrap();
        let k = PathBuf::from(&k[i..]);


        // was getting events for files that didn't exist, so exists() checks manage those
        //  e.g. ./INBOX/4913
        match event.kind {
            EventKind::Create(_) => {
                if !k.exists() { return }

                let md = fs::metadata(&k).unwrap();
                let v = md.modified().unwrap();
                self.index.insert(k.clone(), v);
                print!("[NEW] ");
            }
            EventKind::Modify(_) => {
                if !k.exists() { return }

                let md = fs::metadata(&k).unwrap();
                let v = md.modified().unwrap();
                self.index.insert(k.clone(), v);
                print!("[MOD] ");
            }
            EventKind::Remove(_) => {
                self.index.remove(&k);
                print!("[DEL] ");
            }
            _ => return,
        };

        let num_events = event.paths.len();
        if num_events > 1 { print!("({num_events}) "); }
        println!("{file}", file = k.display());
    }

    fn print(&self) {
        self.index.iter().for_each(|(path, st)| {
            let dt = || -> String {
                let time: DateTime<Local> = DateTime::from(*st);
                time.format("%Y-%m-%d %H:%M").to_string()
            }();
            println!("[{dt}] {file}", file = path.display());
        });
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = init_inbox();
    let directory = PathBuf::from(directory);
    let mut index = FileIndex::new(directory.clone());
    index.print();

    let (tx, mut rx) = tokio::sync::mpsc::channel(1);

    let task = tokio::task::spawn_blocking(async move || {
        let mut watcher = notify::recommended_watcher(move |event| {
            tx.blocking_send(event)
                .expect("couldn't send event over channel");
        })
        .unwrap();
        watcher
            .watch(&directory, RecursiveMode::Recursive)
            .expect("couldn't start monitoring path");

        loop {
            select! {
                _ = signal::ctrl_c() => {
                    break;
                }
                event = rx.recv() => {
                    match event {
                        Some(x) => {
                            index.handle_event(&x.unwrap());
                        },
                        None => {},
                    };
                }
            };
        }

        println!("");
        index.print();
    });

    task.await.unwrap().await;

    Ok(())
}
