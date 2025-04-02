use core::panic;
use std::collections::{HashMap, VecDeque};
use std::env;
use std::fmt::Result;
use std::fs::{read_dir, DirEntry};
use std::panic::Location;
use std::path::{Path, PathBuf};
use std::process::exit;
use std::time::SystemTime;

use chrono::{DateTime, Local};
use futures::future::OptionFuture;
use futures::stream::{StreamExt, FuturesUnordered};
use notify::{Event, EventKind, RecursiveMode, Watcher};
use notify::event::{RemoveKind, CreateKind};
use tokio::{fs, select, signal};

const DEFAULT_INBOX: &'static str = "INBOX/";
const DEFAULT_TARGET: &'static str = "CLONE/";
const DEFAULT_LOG: &'static str = "fm.log";

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

struct Logs {
    log_path: PathBuf,
    file: Option<fs::File>,
}

impl Logs {
    fn new(log_path: Option<&PathBuf>) -> Self {
        let log_path = match log_path {
            Some(path) => path.clone(),
            None => PathBuf::from(DEFAULT_LOG),
        };

        Self {
            log_path,
            file: None
        }
    }

    async fn write(&mut self) {
        if let None = self.file {
            self.file = Some(fs::File::options().append(true).open(&self.log_path).await.unwrap());
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

    if let Ok(false) = std::fs::exists(&directory) {
        std::fs::create_dir(&directory).unwrap();
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
            let dir = read_dir(file.path());
            match dir {
                Err(e) => {
                    eprintln!("WARN: Failed to read dir '{path}': {e}", path = file.path().display());
                },
                Ok(dir) => dir.for_each(|sub_file| cur_files.push_back(sub_file.unwrap())),
            }
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

struct FileIndex {
    index: HashMap<PathBuf, SystemTime>,
    location: PathBuf,
}

impl FileIndex {
    fn new(directory: PathBuf, files: &Option<Vec<String>>) -> Self {
        let mut index: HashMap<PathBuf, SystemTime> = HashMap::new();
        if let Some(files) = files {
            for file in files {
                let md = std::fs::metadata(&file).unwrap();
                let time = md.modified().unwrap();
                index.insert(PathBuf::from(&file), time);
            }
        }
        Self {
            index,
            location: directory,
        }
    }

    // Notify docs specify that there can be more than one file per event, however I haven't
    // observed this. This currently only handles the first file per event, and will display number
    // of events if > 1 event.
    //
    // i think making this async causes a race condition where order of event processing might get
    // fucked up ? but idk lmao
    async fn handle_event(&mut self, event: &Event) -> Option<PathBuf> {
        let k = event.paths[0].to_str().expect("path is not valid unicode");
        let i = k
            .find(self.location.to_str().expect("path is not valid unicode"))
            .unwrap();
        let k = PathBuf::from(&k[i..]);


        // was getting random events for files that dont exist here e.g. ./INBOX/4913
        match event.kind {
            EventKind::Create(kind) => {
                if !k.exists() { return None }
                if kind == CreateKind::Folder { return None }

                let md = fs::metadata(&k).await.unwrap();
                let v = md.modified().unwrap();
                self.index.insert(k.clone(), v);
                print!("[NEW] ");
            }
            EventKind::Modify(_) => {
                if !k.exists() || k.is_dir() { return None }

                let md = fs::metadata(&k).await.unwrap();
                let v = md.modified().unwrap();
                self.index.insert(k.clone(), v);
                print!("[MOD] ");
            }
            EventKind::Remove(kind) => {
                if kind == RemoveKind::Folder { return None }
                self.index.remove(&k);
                print!("[DEL] ");
            }
            _ => return None,
        };

        let num_events = event.paths.len();
        if num_events > 1 { print!("({num_events}) "); }
        println!("{file}", file = k.display());
        Some(k)
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

fn host_path_to_target(host: &PathBuf) -> PathBuf {
    let host = host.to_string_lossy();
    let target = host.replace(DEFAULT_INBOX, DEFAULT_TARGET);

    PathBuf::from(target)
}

// Tries to copy from -> to, path isn't found creates the path
async fn copy_with_dir(from: &PathBuf, to: &PathBuf) {
    let mut target_path = to.clone();
    match fs::copy(&from, &to).await {
        Ok(_) => {},
        Err(err) if err.kind() == tokio::io::ErrorKind::NotFound => {
            target_path.pop();
            if let Err(err) = fs::create_dir_all(&target_path).await {
                panic!("ERROR: couldn't crate path to {path}: {err}", path = target_path.display());
            } else {
                fs::copy(&from, &to).await.expect(format!("path to '{}' was constructed but isn't valid", to.display()).as_str());
            }
        },
        Err(err) => panic!("ERROR: idk: {err}"),
    };
}

// for now, just do async file i/o into clone dir, convert to doing it over ssh later
// precondition: event is one of: new, remove, modify, event is on a file
// initial files are already mirrored
async fn mirror(host_file: &String, event: Option<&Event>) -> tokio::io::Result<()> {
    let host_file = PathBuf::from(&host_file);
    let target_path = host_path_to_target(&host_file);
    let target_file = target_path.clone();


    match event {
        None => copy_with_dir(&host_file, &target_file).await,
        Some(event) => {
            match event.kind {
                EventKind::Create(_) => {
                    copy_with_dir(&host_file, &target_file).await;
                },
                EventKind::Modify(_) => {
                    if host_file.is_file() {
                        fs::copy(host_file, target_file).await?;
                    }
                },
                EventKind::Remove(_) => {
                    fs::remove_file(target_file).await?;
                },
                _ => panic!("Passed invalid event to mirror"),
            }
        },
    };


    Ok(())
}

#[tokio::main]
async fn main() -> Result<> {
    let directory = init_inbox();
    let directory = PathBuf::from(directory);
    let files = find_files_in(&directory);
    let mut index = FileIndex::new(directory.clone(), &files);
    index.print();

    // Clone initial files
    if let Some(files) = files {
        let futures: FuturesUnordered<_> = (&files).into_iter().map(|f| mirror(&f, None)).collect();
        let res: Vec<_> = futures.collect::<Vec<_>>().await;
        for i in res {
            if let Err(e) = i {
                // TODO: log errors to a file here
                eprintln!("WARN: Failed to mirror a file: {e}");
            }
        }
    }

    // Start task to monitor files
    let (tx, mut rx) = tokio::sync::mpsc::channel(10);
    let task = tokio::task::spawn(async move {
        let mut watcher = notify::recommended_watcher(move |event| {
            tx.blocking_send(event)
                .expect("couldn't send event over channel");
        })
        .unwrap();
        // TODO: handle error, watch all available paths
        let res = watcher.watch(&directory, RecursiveMode::Recursive);

        if let Err(e) = res {
            panic!("ERROR: Couldn't watch directoy: {e}");
            // Log error
            // find directories in {directory}, try to watch those instead
        }

        loop {
            select! {
                _ = signal::ctrl_c() => {
                    break;
                }
                event = rx.recv() => {
                    if let Some(x) = event {
                        let x = x.unwrap();
                        let path = index.handle_event(&x).await;
                        if let Some(p) = path {
                            // TODO: error handling here (dont panic)
                            // just log error and continue
                            match mirror(&p.to_string_lossy().to_string(), Some(&x)).await {
                                Err(err) => { eprintln!("Failed to mirror {path}: {err}", path = p.display()) },
                                Ok(_) => {},
                            };
                        }
                    }
                }
            };
        }

        println!("");
        index.print();
    });

    task.await.unwrap();

    Ok(())
}
