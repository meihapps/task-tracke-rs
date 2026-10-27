# task-tracke-rs

a command-line task tracker with short ids, repeating tasks, and full undo and redo.

## installation

```sh
cargo install task-tracke-rs
```

this installs a binary called `todo`.

## usage

```sh
todo add "write report" --due 2d
todo add "water plants" --due-at "2026-11-01 09:00" --repeat 1w
todo add "read a book"
todo
```

```
9       write report due: 2026-10-29 16:23
5       water plants due: 2026-11-01 09:00
e       read a book
```

each task has a short hexadecimal id, which other commands take:

```sh
todo start 9         # mark as in progress
todo complete 9      # mark as complete
todo postpone 5 2d   # push the due date back
todo edit e --name "read two books" --due 1w
todo delete e
todo show 5          # show one task in detail
```

completing a repeating task adds it again, due one repeat later.

ids stay short by growing only when they need to, so adding or deleting a task can change
another task's id by a digit. `todo` shows your tasks after every change, so the current
ids are always on screen.

### showing tasks

`todo` with no arguments is the same as `todo show`, which hides completed tasks by default.

```sh
todo show --all                  # include completed tasks
todo show --status in-progress   # only tasks in progress
todo show --status '!incomplete' # anything except incomplete tasks
todo show --overdue              # only tasks past their due date
todo show --due-within 3d        # due in the next three days, including overdue
```

### undo and redo

every command that changes tasks can be undone, and undone commands can be redone. running
`todo undo` repeatedly steps further back through history, and history is kept indefinitely.
running a new command after undoing discards the commands that could have been redone.

```sh
todo delete 5
todo undo   # the task is back, with its old id
todo redo   # and deleted again
```

### value formats

run `todo --help` for a summary of every value format.

| value      | format                                                                 | examples                     |
| ---------- | ---------------------------------------------------------------------- | ---------------------------- |
| id         | hexadecimal, as shown by `todo show`                                   | `3`, `a7`                    |
| name       | any non-blank text. quote names containing spaces                      | `"write report"`             |
| duration   | numbers with units: `s`, `m` (minutes), `h`, `d`, `w`, `month`, `y`    | `30m`, `2h`, `1w2d`, `1month` |
| date       | local time as `yyyy-mm-dd hh:mm`, or `yyyy-mm-dd` for midnight         | `"2026-11-01 09:00"`, `2026-11-01` |
| status     | `incomplete`, `in-progress` or `complete`, with a leading `!` to negate | `in-progress`, `'!complete'` |

durations with spaces, such as `"1w 2d"`, also work when quoted.

### deleting everything

```sh
todo purge --unsafe
```

this permanently deletes all tasks and undo history. it cannot be undone, and does nothing
without `--unsafe`.

## data

tasks and history are stored in a `task-tracke-rs` directory inside the local configuration
directory:

| platform | location                                           |
| -------- | -------------------------------------------------- |
| linux    | `$XDG_CONFIG_HOME/task-tracke-rs` or `~/.config/task-tracke-rs` |
| macos    | `~/Library/Application Support/task-tracke-rs`     |
| windows  | `%LOCALAPPDATA%\task-tracke-rs`                    |

the directory holds `tasks.json`, and `undo.jsonl` and `redo.jsonl` for history.

## licence

licensed under either of

- apache license, version 2.0 ([license-apache](LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
- mit licence ([license-mit](LICENSE-MIT) or <http://opensource.org/licenses/mit>)

at your option.

unless you explicitly state otherwise, any contribution intentionally submitted for inclusion
in the work by you, as defined in the apache-2.0 licence, shall be dual licensed as above,
without any additional terms or conditions.
