```
pssh
* file propagation/mirroring over openssh


Target can be specified in whatever the rust equivalent of make's configure is,
so that it doesn't have to be specified more than once, maybe multiple hosts
could be specified:

OR (more likely)

can just have a config file that is manually written to


can be split into some phases seen below

Initial propagation (sync):
    1. read config file
        * has ssh connection information
    2. open ssh connections
    3. copy directory contents from HOST to TARGET
        * consider when directory on TARGET isn't empty

Monitoring (async):
    1. monitor changes in HOST dir
    2. log changes to stdout + log file
    3. propagate any changes detected on HOST to all TARGETS

Cleanup (sync):
    1. close connections
    2. give a summary of changes made
        * maybe just dump file index

Notes
- if file exists on target, ask for confirmation
    * (option to disable this and just propagate everything)
- HOST does not track changes to TARGETS files, it's only responsible for
  reflecting the changes done on HOST to all TARGETS


idea originated from here:
https://codereview.stackexchange.com/questions/294623/file-list-and-monitor

======= >> plaintext in case post gets taken down
    Create a command-line program that accepts an optional argument: “-d path”. If
    the path is not supplied, it defaults to “~/inbox/”. If the path does not
    exist, it should be created. We refer to this path as “INBOX” in the rest of
    the document.

    Program workflow:

        1. Scan the folder recursively and print to stdout all the files found and
        their last modification date in the following format: “[Date Time] PATH”, where
        PATH is a relative path to INBOX.

        2. Start monitoring INBOX for file changes. When an event occurs, print it to
        stdout in the following format: “[EVENT] PATH”, where EVENT is one of the
        following [NEW, MOD, DEL].

        3. Continue monitoring until the user inputs Ctrl-C.

        4. Once Ctrl-C is detected, print to stdout the contents of INBOX again in the
        same format, without rescanning or any other FS operations.

    Bonus points for:
        1. Using tokio
        2. Using structured error handling
        3. Not using mutexes
        4. Having separation of concerns
<< =======


```
