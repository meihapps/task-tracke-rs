use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Duration, Local};
use clap::{ArgGroup, Args, Parser, Subcommand};
use dirs::config_local_dir;
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::{BufReader, BufWriter, ErrorKind, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};
use task_tracke_rs::{
    Record, StatusFilter, StatusKind, Task, TaskEdit, TaskFilter, TaskId, TaskTree, parse_due_at,
    parse_due_in, parse_name, parse_time_delta,
};

/// A command-line task tracker with undo and redo.
#[derive(Parser)]
#[command(name = "todo", version, after_help = FORMATS)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

const FORMATS: &str = "\
Value formats:
  <ID>        A task's id, as shown by `todo show`, in hexadecimal, e.g. 3 or a7
  <NAME>      Any non-blank text. Quote names containing spaces, e.g. \"write report\"
  <DURATION>  Numbers with units, e.g. 30m, 2h, 3d, 1w2d or 1month. Quote any with spaces
              Units: s, m (minutes), h, d, w, month or M, y
  <DATE>      Local time as 'YYYY-MM-DD HH:MM', or 'YYYY-MM-DD' for midnight
  <STATUS>    incomplete, in-progress or complete, with a leading ! to negate, e.g. '!complete'";

#[derive(Subcommand)]
enum Commands {
    /// Show tasks, or one task in detail. This is the default command.
    Show(ShowArgs),
    /// Add a task.
    Add(AddArgs),
    /// Change a task's name, due date or repeat.
    Edit(EditArgs),
    /// Mark a task as in progress.
    Start(StartArgs),
    /// Mark a task as complete. Repeating tasks are added again with their next due date.
    Complete(CompleteArgs),
    /// Delete a task.
    Delete(DeleteArgs),
    /// Push a task's due date back.
    Postpone(PostponeArgs),
    /// Undo the last command that changed tasks.
    Undo,
    /// Redo the last undone command.
    Redo,
    /// Permanently delete all tasks and undo history.
    Purge(PurgeArgs),
}

#[derive(Args, Default)]
struct ShowArgs {
    /// Show this task in detail, e.g. 3a.
    #[arg(
        value_name = "ID",
        conflicts_with_all = ["status", "all", "overdue", "due_within"]
    )]
    id: Option<TaskId>,

    /// Filter by status: incomplete, in-progress or complete, negated with a leading '!'.
    /// Defaults to '!complete'. Quote negations, as '!' is special in many shells.
    #[arg(long, value_name = "STATUS", conflicts_with = "all")]
    status: Option<StatusFilter>,

    /// Show tasks of every status.
    #[arg(long)]
    all: bool,

    /// Only show tasks whose due date has passed.
    #[arg(long, conflicts_with = "due_within")]
    overdue: bool,

    /// Only show tasks due within the given duration, including overdue tasks, e.g. 3d.
    #[arg(long, value_name = "DURATION", value_parser = parse_due_in)]
    due_within: Option<DateTime<Local>>,
}

impl ShowArgs {
    fn filter(&self) -> TaskFilter {
        let status = if self.all {
            None
        } else {
            Some(
                self.status
                    .unwrap_or(StatusFilter::Not(StatusKind::Complete)),
            )
        };

        let due_before = if self.overdue {
            Some(Local::now())
        } else {
            self.due_within
        };

        TaskFilter { status, due_before }
    }
}

#[derive(Args)]
#[command(group(ArgGroup::new("due_date").args(["due", "due_at"])))]
struct AddArgs {
    /// The task's name. Quote names containing spaces, e.g. "write report".
    #[arg(value_name = "NAME", value_parser = parse_name)]
    name: String,

    /// Due the given duration from now, e.g. 2h, 3d or 1w2d.
    #[arg(long, value_name = "DURATION", value_parser = parse_due_in)]
    due: Option<DateTime<Local>>,

    /// Due at the given local time, as 'YYYY-MM-DD HH:MM' or 'YYYY-MM-DD' for midnight.
    #[arg(long, value_name = "DATE", value_parser = parse_due_at)]
    due_at: Option<DateTime<Local>>,

    /// When completed, add the task again due this much later, e.g. 1d or 1w. Requires a due date.
    #[arg(
        long,
        value_name = "DURATION",
        requires = "due_date",
        value_parser = parse_time_delta
    )]
    repeat: Option<Duration>,
}

#[derive(Args)]
#[command(group(
    ArgGroup::new("changes")
        .required(true)
        .multiple(true)
        .args(["name", "due", "due_at", "no_due", "repeat", "no_repeat"])
))]
#[command(group(ArgGroup::new("due_change").args(["due", "due_at", "no_due"])))]
#[command(group(ArgGroup::new("repeat_change").args(["repeat", "no_repeat"])))]
struct EditArgs {
    /// The task to edit, e.g. 3a.
    #[arg(value_name = "ID")]
    id: TaskId,

    /// Rename the task. Quote names containing spaces, e.g. "write report".
    #[arg(long, value_name = "NAME", value_parser = parse_name)]
    name: Option<String>,

    /// Set the due date to the given duration from now, e.g. 2h, 3d or 1w2d.
    #[arg(long, value_name = "DURATION", value_parser = parse_due_in)]
    due: Option<DateTime<Local>>,

    /// Set the due date to the given local time, as 'YYYY-MM-DD HH:MM' or 'YYYY-MM-DD' for
    /// midnight.
    #[arg(long, value_name = "DATE", value_parser = parse_due_at)]
    due_at: Option<DateTime<Local>>,

    /// Remove the due date.
    #[arg(long)]
    no_due: bool,

    /// Set how much later the task is due again once completed, e.g. 1d or 1w.
    #[arg(long, value_name = "DURATION", value_parser = parse_time_delta)]
    repeat: Option<Duration>,

    /// Remove the repeat.
    #[arg(long)]
    no_repeat: bool,
}

impl EditArgs {
    fn edit(&self) -> TaskEdit {
        let due = if self.no_due {
            Some(None)
        } else {
            self.due_at.or(self.due).map(Some)
        };

        let repeat = if self.no_repeat {
            Some(None)
        } else {
            self.repeat.map(Some)
        };

        TaskEdit {
            name: self.name.clone(),
            due,
            repeat,
        }
    }
}

#[derive(Args)]
struct StartArgs {
    /// The task to start, e.g. 3a.
    #[arg(value_name = "ID")]
    id: TaskId,
}

#[derive(Args)]
struct CompleteArgs {
    /// The task to complete, e.g. 3a.
    #[arg(value_name = "ID")]
    id: TaskId,
}

#[derive(Args)]
struct DeleteArgs {
    /// The task to delete, e.g. 3a.
    #[arg(value_name = "ID")]
    id: TaskId,
}

#[derive(Args)]
struct PostponeArgs {
    /// The task to postpone, e.g. 3a.
    #[arg(value_name = "ID")]
    id: TaskId,
    /// How long to postpone by, e.g. 2h or 1w2d. Overdue tasks are postponed from now.
    #[arg(value_name = "DURATION", value_parser = parse_time_delta)]
    duration: Duration,
}

#[derive(Args)]
struct PurgeArgs {
    /// Confirm permanently deleting all tasks and history. This cannot be undone.
    #[arg(long = "unsafe")]
    confirmed: bool,
}

/// The task tree, and how many records are on the undo stack.
#[derive(Default, Serialize, Deserialize)]
struct State {
    undo_len: u64,
    tree: TaskTree,
}

/// A record on an undo or redo stack. `seq` is the record's position in the undo stack, so
/// the record on top of the undo stack has `seq == undo_len - 1`, and the record on top of
/// the redo stack has `seq == undo_len`.
#[derive(Serialize, Deserialize)]
struct Entry {
    seq: u64,
    record: Record,
}

/// A stack of entries stored one per line, pushed and popped at the end of the file.
struct Stack(PathBuf);

/// The saved state and its undo and redo stacks.
///
/// `tasks.json` is the commit point: each operation writes to the stacks before it, and any
/// stack write left over from an interrupted operation is detectable by its `seq` and
/// discarded by [`Store::reconcile`].
struct Store {
    dir: PathBuf,
    tasks: PathBuf,
    undo: Stack,
    redo: Stack,
}

impl Stack {
    fn push(&self, entry: &Entry) -> Result<()> {
        let mut line = serde_json::to_vec(entry)?;
        line.push(b'\n');
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.0)?
            .write_all(&line)?;
        Ok(())
    }

    fn peek(&self) -> Result<Option<Entry>> {
        let Some((mut file, start, end)) = self.last_line()? else {
            return Ok(None);
        };

        let mut line = vec![0; usize::try_from(end - start)?];
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(&mut line)?;

        let entry = serde_json::from_slice(&line)
            .with_context(|| format!("Corrupt history in '{}'.", self.0.display()))?;
        Ok(Some(entry))
    }

    fn peek_seq(&self) -> Result<Option<u64>> {
        Ok(self.peek()?.map(|entry| entry.seq))
    }

    fn pop(&self) -> Result<()> {
        if let Some((file, start, _)) = self.last_line()? {
            file.set_len(start)?;
        }
        Ok(())
    }

    fn clear(&self) -> Result<()> {
        remove_if_exists(&self.0)
    }

    /// Opens the stack and finds the bounds of its last line, if any. A trailing partial line,
    /// left by an interrupted push, is discarded first.
    fn last_line(&self) -> Result<Option<(File, u64, u64)>> {
        let mut file = match OpenOptions::new().read(true).write(true).open(&self.0) {
            Ok(file) => file,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err.into()),
        };

        let mut end = file.metadata()?.len();
        if end > 0 {
            let mut last = [0];
            file.seek(SeekFrom::Start(end - 1))?;
            file.read_exact(&mut last)?;
            if last[0] != b'\n' {
                end = line_start(&mut file, end)?;
                file.set_len(end)?;
            }
        }

        if end == 0 {
            return Ok(None);
        }

        // Skip the last line's own newline.
        let start = line_start(&mut file, end - 1)?;
        Ok(Some((file, start, end)))
    }
}

/// Returns the position just after the last newline before `end`, or 0 if there is none.
fn line_start(file: &mut File, mut end: u64) -> Result<u64> {
    const CHUNK: u64 = 4096;
    let mut buf = Vec::new();

    while end > 0 {
        let start = end.saturating_sub(CHUNK);
        buf.resize(usize::try_from(end - start)?, 0);
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(&mut buf)?;

        if let Some(i) = buf.iter().rposition(|&b| b == b'\n') {
            return Ok(start + u64::try_from(i)? + 1);
        }
        end = start;
    }

    Ok(0)
}

fn remove_if_exists(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Err(err) if err.kind() != ErrorKind::NotFound => Err(err.into()),
        _ => Ok(()),
    }
}

impl Store {
    fn new(dir: PathBuf) -> Self {
        Self {
            tasks: dir.join("tasks.json"),
            undo: Stack(dir.join("undo.jsonl")),
            redo: Stack(dir.join("redo.jsonl")),
            dir,
        }
    }

    /// Loads the saved state, or an empty state if there is none.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The state file exists but cannot be read.
    /// - Deserialisation of the state fails.
    fn load(&self) -> Result<State> {
        let file = match File::open(&self.tasks) {
            Ok(file) => file,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(State::default()),
            Err(err) => return Err(err.into()),
        };

        let value: serde_json::Value = serde_json::from_reader(BufReader::new(file))?;

        // Files saved before undo history existed hold a bare task tree.
        if value.get("children").is_some() {
            let mut tree: TaskTree = serde_json::from_value(value)?;
            tree.optimise();
            return Ok(State { undo_len: 0, tree });
        }

        Ok(serde_json::from_value(value)?)
    }

    /// Saves the state, replacing the old file atomically.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The directory does not exist and cannot be created.
    /// - Serialisation or writing of the state fails.
    fn save(&self, state: &State) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;

        let temp = self.tasks.with_extension("json.tmp");
        let mut writer = BufWriter::new(File::create(&temp)?);
        serde_json::to_writer_pretty(&mut writer, state)?;
        writer.flush()?;
        writer.get_ref().sync_all()?;

        std::fs::rename(&temp, &self.tasks)?;
        Ok(())
    }

    /// Saves the result of a new command, clearing the redo stack.
    fn commit(&self, state: &mut State, record: Record) -> Result<()> {
        self.reconcile(state)?;
        std::fs::create_dir_all(&self.dir)?;

        self.redo.clear()?;
        self.undo.push(&Entry {
            seq: state.undo_len,
            record,
        })?;
        state.undo_len += 1;
        self.save(state)
    }

    fn undo(&self, state: &mut State) -> Result<()> {
        self.reconcile(state)?;
        let entry = self.undo.peek()?.context("Nothing to undo.")?;

        let record = state.tree.apply_record(entry.record)?;
        self.redo.push(&Entry {
            seq: entry.seq,
            record,
        })?;
        state.undo_len -= 1;
        self.save(state)?;
        self.undo.pop()
    }

    fn redo(&self, state: &mut State) -> Result<()> {
        self.reconcile(state)?;
        let entry = self.redo.peek()?.context("Nothing to redo.")?;

        let record = state.tree.apply_record(entry.record)?;
        self.undo.push(&Entry {
            seq: entry.seq,
            record,
        })?;
        state.undo_len += 1;
        self.save(state)?;
        self.redo.pop()
    }

    /// Discards stack writes left over from an interrupted operation, then checks that both
    /// stacks line up with the saved state.
    fn reconcile(&self, state: &State) -> Result<()> {
        // Pushed by an uncommitted command or redo, or left unpopped by a committed undo.
        if self.undo.peek_seq()? == Some(state.undo_len) {
            self.undo.pop()?;
        }
        // Pushed by an uncommitted undo, or left unpopped by a committed redo.
        if state.undo_len > 0 && self.redo.peek_seq()? == Some(state.undo_len - 1) {
            self.redo.pop()?;
        }

        let in_sync = self.undo.peek_seq()? == state.undo_len.checked_sub(1)
            && self
                .redo
                .peek_seq()?
                .is_none_or(|seq| seq == state.undo_len);
        ensure!(
            in_sync,
            "Undo history in '{}' does not match the saved tasks.",
            self.dir.display()
        );
        Ok(())
    }

    fn purge(&self) -> Result<()> {
        remove_if_exists(&self.tasks)?;
        self.undo.clear()?;
        self.redo.clear()
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let store = Store::new(
        config_local_dir()
            .context("Unable to determine local configuration directory.")?
            .join("task-tracke-rs"),
    );

    let command = cli.command.unwrap_or(Commands::Show(ShowArgs::default()));

    if let Commands::Purge(args) = &command {
        ensure!(
            args.confirmed,
            "Purge permanently deletes all tasks and undo history, and cannot be undone. \
             Run it with --unsafe to proceed."
        );
        store.purge()?;
        println!("Deleted all tasks and undo history.");
        return Ok(());
    }

    let mut state = store.load()?;

    let record = match &command {
        Commands::Show(args) => {
            match &args.id {
                Some(id) => println!("{}", state.tree.get_task(id.as_ref())?),
                None => print!("{}", state.tree.filtered(&args.filter())),
            }
            return Ok(());
        }
        Commands::Undo => {
            store.undo(&mut state)?;
            None
        }
        Commands::Redo => {
            store.redo(&mut state)?;
            None
        }
        Commands::Add(args) => Some(state.tree.add_task(Task::new(
            args.name.clone(),
            args.due_at.or(args.due),
            args.repeat,
        )?)),
        Commands::Edit(args) => Some(state.tree.edit_task(args.id.as_ref(), args.edit())?),
        Commands::Start(args) => Some(state.tree.start_task(args.id.as_ref())?),
        Commands::Complete(args) => Some(state.tree.complete_task(args.id.as_ref())?),
        Commands::Delete(args) => Some(state.tree.delete_task(args.id.as_ref())?),
        Commands::Postpone(args) => {
            Some(state.tree.postpone_task(args.id.as_ref(), args.duration)?)
        }
        Commands::Purge(_) => unreachable!("Purge was handled above."),
    };

    if let Some(record) = record {
        store.commit(&mut state, record)?;
    }

    print!("{}", state.tree.filtered(&ShowArgs::default().filter()));

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use task_tracke_rs::Task;
    use tempfile::TempDir;

    fn record(name: &str) -> Record {
        TaskTree::new().add_task(Task::new(name.to_owned(), None, None).unwrap())
    }

    fn entry(seq: u64) -> Entry {
        Entry {
            seq,
            record: record("x"),
        }
    }

    fn stack(dir: &TempDir) -> Stack {
        Stack(dir.path().join("stack.jsonl"))
    }

    fn store(dir: &TempDir) -> Store {
        Store::new(dir.path().join("task-tracke-rs"))
    }

    fn tree_json(state: &State) -> String {
        serde_json::to_string(&state.tree).unwrap()
    }

    /// Runs `count` new commands, each adding a task.
    fn add_tasks(store: &Store, state: &mut State, count: usize) {
        for i in 0..count {
            let record = state
                .tree
                .add_task(Task::new(format!("t{i}"), None, None).unwrap());
            store.commit(state, record).unwrap();
        }
    }

    fn seqs(stack: &Stack) -> Vec<u64> {
        std::fs::read_to_string(&stack.0)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str::<Entry>(line).unwrap().seq)
            .collect()
    }

    // Stack.

    #[test]
    fn missing_stack_is_empty() {
        let dir = TempDir::new().unwrap();
        let stack = stack(&dir);
        assert!(stack.peek().unwrap().is_none());
        stack.pop().unwrap();
        stack.clear().unwrap();
    }

    #[test]
    fn stack_pushes_and_pops_in_order() {
        let dir = TempDir::new().unwrap();
        let stack = stack(&dir);
        for seq in 0..3 {
            stack.push(&entry(seq)).unwrap();
        }

        for seq in (0..3).rev() {
            assert_eq!(stack.peek_seq().unwrap(), Some(seq));
            stack.pop().unwrap();
        }
        assert!(stack.peek().unwrap().is_none());
        assert_eq!(std::fs::metadata(&stack.0).unwrap().len(), 0);
    }

    #[test]
    fn stack_handles_lines_longer_than_a_chunk() {
        let dir = TempDir::new().unwrap();
        let stack = stack(&dir);
        let long = Entry {
            seq: 1,
            record: record(&"x".repeat(10_000)),
        };
        stack.push(&entry(0)).unwrap();
        stack.push(&long).unwrap();
        stack.push(&long).unwrap();

        stack.pop().unwrap();
        assert_eq!(stack.peek_seq().unwrap(), Some(1));
        stack.pop().unwrap();
        assert_eq!(stack.peek_seq().unwrap(), Some(0));
        stack.pop().unwrap();
        assert!(stack.peek().unwrap().is_none());
    }

    #[test]
    fn stack_discards_partial_last_line() {
        let dir = TempDir::new().unwrap();
        let stack = stack(&dir);
        stack.push(&entry(0)).unwrap();
        stack.push(&entry(1)).unwrap();
        OpenOptions::new()
            .append(true)
            .open(&stack.0)
            .unwrap()
            .write_all(br#"{"seq":2,"rec"#)
            .unwrap();

        assert_eq!(stack.peek_seq().unwrap(), Some(1));
        assert_eq!(seqs(&stack), [0, 1]);
    }

    #[test]
    fn stack_discards_partial_only_line() {
        let dir = TempDir::new().unwrap();
        let stack = stack(&dir);
        std::fs::write(&stack.0, r#"{"seq":0"#).unwrap();
        assert!(stack.peek().unwrap().is_none());
        assert_eq!(std::fs::metadata(&stack.0).unwrap().len(), 0);
    }

    #[test]
    fn corrupt_stack_line_is_an_error() {
        let dir = TempDir::new().unwrap();
        let stack = stack(&dir);
        std::fs::write(&stack.0, "not json\n").unwrap();
        assert!(stack.peek().is_err());
    }

    #[test]
    fn line_start_finds_line_boundaries() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("lines");
        let text = format!("a\n{}\nb\n", "x".repeat(5000));
        std::fs::write(&path, &text).unwrap();
        let mut file = File::open(&path).unwrap();
        let len = u64::try_from(text.len()).unwrap();

        assert_eq!(line_start(&mut file, len).unwrap(), len);
        assert_eq!(line_start(&mut file, len - 1).unwrap(), len - 2);
        assert_eq!(line_start(&mut file, len - 2).unwrap(), len - 2);
        // Searches back across a chunk boundary to the newline after `a`.
        assert_eq!(line_start(&mut file, len - 3).unwrap(), 2);
        assert_eq!(line_start(&mut file, 1).unwrap(), 0);
        assert_eq!(line_start(&mut file, 0).unwrap(), 0);
    }

    // Store.

    #[test]
    fn load_without_saved_state_is_empty() {
        let dir = TempDir::new().unwrap();
        let state = store(&dir).load().unwrap();
        assert_eq!(state.undo_len, 0);
        assert_eq!(
            tree_json(&state),
            serde_json::to_string(&TaskTree::new()).unwrap()
        );
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = TempDir::new().unwrap();
        let store = store(&dir);
        let mut state = State::default();
        add_tasks(&store, &mut state, 3);

        let loaded = store.load().unwrap();
        assert_eq!(loaded.undo_len, 3);
        assert_eq!(tree_json(&loaded), tree_json(&state));
        assert!(!store.tasks.with_extension("json.tmp").exists());
    }

    #[test]
    fn load_migrates_legacy_tree() {
        let dir = TempDir::new().unwrap();
        let store = store(&dir);
        std::fs::create_dir_all(&store.dir).unwrap();

        // `3: tree { 5: tree { 1: a, 2: b } }`, a chain older versions could produce.
        let task = |name: &str| {
            serde_json::json!({ "Task": {
                "creation_date": "2030-01-01T00:00:00Z",
                "due": null,
                "name": name,
                "status": "Incomplete",
                "repeat": null,
            }})
        };
        let tree = |children: Vec<(usize, serde_json::Value)>| {
            let mut slots = vec![serde_json::Value::Null; 16];
            for (i, child) in children {
                slots[i] = child;
            }
            serde_json::json!({ "children": slots })
        };
        let inner = tree(vec![(1, task("a")), (2, task("b"))]);
        let middle = tree(vec![(5, serde_json::json!({ "Tree": inner }))]);
        let root = tree(vec![(3, serde_json::json!({ "Tree": middle }))]);
        std::fs::write(&store.tasks, root.to_string()).unwrap();

        let state = store.load().unwrap();
        assert_eq!(state.undo_len, 0);
        let ids: Vec<String> = state
            .tree
            .collect_tasks(&TaskId::new())
            .into_iter()
            .map(|(id, _)| id.to_string())
            .collect();
        assert_eq!(ids, ["31", "32"]);
    }

    #[test]
    fn corrupt_state_is_an_error() {
        let dir = TempDir::new().unwrap();
        let store = store(&dir);
        std::fs::create_dir_all(&store.dir).unwrap();
        std::fs::write(&store.tasks, r#"{"undo_len": "x"}"#).unwrap();
        assert!(store.load().is_err());
    }

    #[test]
    fn commit_undo_and_redo_keep_stacks_in_step() {
        let dir = TempDir::new().unwrap();
        let store = store(&dir);
        let mut state = State::default();
        let mut snapshots = vec![tree_json(&state)];
        for i in 1..=3 {
            add_tasks(&store, &mut state, 1);
            snapshots.push(tree_json(&state));
            assert_eq!(state.undo_len, i);
        }
        assert_eq!(seqs(&store.undo), [0, 1, 2]);

        for len in (0..3).rev() {
            store.undo(&mut state).unwrap();
            assert_eq!(state.undo_len, len);
            assert_eq!(tree_json(&state), snapshots[usize::try_from(len).unwrap()]);
        }
        assert!(store.undo(&mut state).is_err());
        assert_eq!(seqs(&store.undo), [] as [u64; 0]);
        assert_eq!(seqs(&store.redo), [2, 1, 0]);

        for len in 1..=3 {
            store.redo(&mut state).unwrap();
            assert_eq!(state.undo_len, len);
            assert_eq!(tree_json(&state), snapshots[usize::try_from(len).unwrap()]);
        }
        assert!(store.redo(&mut state).is_err());
        assert_eq!(seqs(&store.undo), [0, 1, 2]);

        let saved = store.load().unwrap();
        assert_eq!(saved.undo_len, 3);
        assert_eq!(tree_json(&saved), snapshots[3]);
    }

    #[test]
    fn nothing_to_undo_or_redo_changes_nothing() {
        let dir = TempDir::new().unwrap();
        let store = store(&dir);
        let mut state = State::default();
        assert!(store.undo(&mut state).is_err());
        assert!(store.redo(&mut state).is_err());
        assert!(!store.tasks.exists());
    }

    #[test]
    fn new_command_clears_redo() {
        let dir = TempDir::new().unwrap();
        let store = store(&dir);
        let mut state = State::default();
        add_tasks(&store, &mut state, 2);
        store.undo(&mut state).unwrap();
        assert_eq!(seqs(&store.redo), [1]);

        add_tasks(&store, &mut state, 1);
        assert!(!store.redo.0.exists());
        assert_eq!(seqs(&store.undo), [0, 1]);
        assert!(store.redo(&mut state).is_err());
    }

    #[test]
    fn reconcile_drops_push_from_interrupted_command() {
        let dir = TempDir::new().unwrap();
        let store = store(&dir);
        let mut state = State::default();
        add_tasks(&store, &mut state, 2);
        let committed = tree_json(&state);

        // The command pushed its record, but never saved the state.
        store.undo.push(&entry(2)).unwrap();

        let mut state = store.load().unwrap();
        store.undo(&mut state).unwrap();
        assert_eq!(state.undo_len, 1);
        store.redo(&mut state).unwrap();
        assert_eq!(tree_json(&state), committed);
    }

    #[test]
    fn reconcile_drops_push_from_interrupted_undo() {
        let dir = TempDir::new().unwrap();
        let store = store(&dir);
        let mut state = State::default();
        add_tasks(&store, &mut state, 2);
        store.undo(&mut state).unwrap();

        // An undo of record 0 pushed its inverse, but never saved the state.
        store.redo.push(&entry(0)).unwrap();

        let mut state = store.load().unwrap();
        store.redo(&mut state).unwrap();
        assert_eq!(state.undo_len, 2);
        assert!(store.redo(&mut state).is_err());
    }

    #[test]
    fn reconcile_drops_unpopped_entry_after_undo_or_redo() {
        let dir = TempDir::new().unwrap();
        let store = store(&dir);
        let mut state = State::default();
        add_tasks(&store, &mut state, 2);

        // An undo saved the state, but never popped the undo stack.
        store.undo(&mut state).unwrap();
        store.undo.push(&entry(1)).unwrap();
        let mut state = store.load().unwrap();
        store.undo(&mut state).unwrap();
        assert_eq!(state.undo_len, 0);

        // A redo saved the state, but never popped the redo stack.
        store.redo(&mut state).unwrap();
        store.redo.push(&entry(0)).unwrap();
        let mut state = store.load().unwrap();
        store.redo(&mut state).unwrap();
        assert_eq!(state.undo_len, 2);
        assert!(store.redo(&mut state).is_err());
    }

    #[test]
    fn mismatched_history_is_an_error() {
        let dir = TempDir::new().unwrap();
        let store = store(&dir);
        let mut state = State::default();
        add_tasks(&store, &mut state, 2);
        store.undo.push(&entry(7)).unwrap();

        let mut state = store.load().unwrap();
        let before = tree_json(&state);
        assert!(store.undo(&mut state).is_err());
        let record = state
            .tree
            .add_task(Task::new("x".to_owned(), None, None).unwrap());
        assert!(store.commit(&mut state, record).is_err());
        assert_eq!(tree_json(&store.load().unwrap()), before);
    }

    #[test]
    fn missing_undo_history_is_an_error() {
        let dir = TempDir::new().unwrap();
        let store = store(&dir);
        let mut state = State::default();
        add_tasks(&store, &mut state, 2);
        std::fs::remove_file(&store.undo.0).unwrap();

        let mut state = store.load().unwrap();
        assert!(store.undo(&mut state).is_err());
    }

    #[test]
    fn purge_deletes_everything() {
        let dir = TempDir::new().unwrap();
        let store = store(&dir);
        let mut state = State::default();
        add_tasks(&store, &mut state, 2);
        store.undo(&mut state).unwrap();

        store.purge().unwrap();
        assert!(!store.tasks.exists());
        assert!(!store.undo.0.exists());
        assert!(!store.redo.0.exists());

        let mut state = store.load().unwrap();
        assert_eq!(state.undo_len, 0);
        assert!(store.undo(&mut state).is_err());
        assert!(store.redo(&mut state).is_err());
        store.purge().unwrap();
    }

    // Command line.

    #[test]
    fn cli_definition_is_valid() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
        assert_eq!(Cli::command().get_name(), "todo");
    }

    #[test]
    fn purge_requires_unsafe_flag() {
        let Some(Commands::Purge(args)) = Cli::try_parse_from(["todo", "purge"]).unwrap().command
        else {
            panic!("expected purge");
        };
        assert!(!args.confirmed);

        let Some(Commands::Purge(args)) = Cli::try_parse_from(["todo", "purge", "--unsafe"])
            .unwrap()
            .command
        else {
            panic!("expected purge");
        };
        assert!(args.confirmed);
    }

    #[test]
    fn edit_requires_a_change_and_rejects_conflicts() {
        assert!(Cli::try_parse_from(["todo", "edit", "3"]).is_err());
        assert!(Cli::try_parse_from(["todo", "edit", "3", "--due", "1d", "--no-due"]).is_err());
        assert!(
            Cli::try_parse_from(["todo", "edit", "3", "--repeat", "1d", "--no-repeat"]).is_err()
        );

        let Some(Commands::Edit(args)) =
            Cli::try_parse_from(["todo", "edit", "3", "--no-due", "--no-repeat"])
                .unwrap()
                .command
        else {
            panic!("expected edit");
        };
        let edit = args.edit();
        assert_eq!(edit.due, Some(None));
        assert_eq!(edit.repeat, Some(None));
        assert!(edit.name.is_none());
    }

    #[test]
    fn add_repeat_requires_due_date() {
        assert!(Cli::try_parse_from(["todo", "add", "x", "--repeat", "1d"]).is_err());
        assert!(Cli::try_parse_from(["todo", "add", "x", "--due", "1d", "--repeat", "1d"]).is_ok());
        assert!(
            Cli::try_parse_from(["todo", "add", "x", "--due", "1d", "--due-at", "2030-01-01"])
                .is_err()
        );
        assert!(Cli::try_parse_from(["todo", "add", " "]).is_err());
    }

    #[test]
    fn show_filter_defaults_to_hiding_completed() {
        let filter = ShowArgs::default().filter();
        assert!(matches!(
            filter.status,
            Some(StatusFilter::Not(StatusKind::Complete))
        ));
        assert!(filter.due_before.is_none());

        let Some(Commands::Show(args)) =
            Cli::try_parse_from(["todo", "show", "--all", "--overdue"])
                .unwrap()
                .command
        else {
            panic!("expected show");
        };
        let filter = args.filter();
        assert!(filter.status.is_none());
        assert!(filter.due_before.is_some());
    }
}
