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

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    List(ListArgs),
    Add(AddArgs),
    Edit(EditArgs),
    Start(StartArgs),
    Complete(CompleteArgs),
    Delete(DeleteArgs),
    Postpone(PostponeArgs),
    Undo,
    Redo,
    Purge(PurgeArgs),
}

#[derive(Args, Default)]
struct ListArgs {
    #[arg(conflicts_with_all = ["status", "all", "overdue", "due_within"])]
    id: Option<TaskId>,

    /// Filter by status: incomplete, in-progress or complete, negated with a leading '!'.
    /// Defaults to !complete.
    #[arg(long, conflicts_with = "all")]
    status: Option<StatusFilter>,

    /// Show tasks of every status.
    #[arg(long)]
    all: bool,

    /// Only show tasks whose due date has passed.
    #[arg(long, conflicts_with = "due_within")]
    overdue: bool,

    /// Only show tasks due within the given duration, including overdue tasks.
    #[arg(long, value_parser = parse_due_in)]
    due_within: Option<DateTime<Local>>,
}

impl ListArgs {
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
    #[arg(value_parser = parse_name)]
    name: String,

    /// Due in the given duration from now.
    #[arg(long, value_parser = parse_due_in)]
    due: Option<DateTime<Local>>,

    /// Due at the given local time: 'YYYY-MM-DD HH:MM' or 'YYYY-MM-DD' (midnight).
    #[arg(long, value_parser = parse_due_at)]
    due_at: Option<DateTime<Local>>,

    /// Repeat by the given duration on completion. Requires a due date.
    #[arg(long, requires = "due_date", value_parser = parse_time_delta)]
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
    id: TaskId,

    #[arg(long, value_parser = parse_name)]
    name: Option<String>,

    /// Set the due date to the given duration from now.
    #[arg(long, value_parser = parse_due_in)]
    due: Option<DateTime<Local>>,

    /// Set the due date to the given local time: 'YYYY-MM-DD HH:MM' or 'YYYY-MM-DD' (midnight).
    #[arg(long, value_parser = parse_due_at)]
    due_at: Option<DateTime<Local>>,

    /// Remove the due date.
    #[arg(long)]
    no_due: bool,

    /// Set the repeat duration.
    #[arg(long, value_parser = parse_time_delta)]
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
    id: TaskId,
}

#[derive(Args)]
struct CompleteArgs {
    id: TaskId,
}

#[derive(Args)]
struct DeleteArgs {
    id: TaskId,
}

#[derive(Args)]
struct PostponeArgs {
    id: TaskId,
    #[arg(value_parser = parse_time_delta)]
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

    let command = cli.command.unwrap_or(Commands::List(ListArgs::default()));

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
        Commands::List(args) => {
            match &args.id {
                Some(id) => print!("{}", state.tree.get_task(id.as_ref())?),
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

    print!("{}", state.tree.filtered(&ListArgs::default().filter()));

    Ok(())
}
