use anyhow::Result;
use chrono::{DateTime, Local};
use clap::{Args, Parser, Subcommand};
use dirs::config_local_dir;
use humantime::Duration;
use std::{
    fs::File,
    io::{BufReader, BufWriter},
    path::Path,
};
use task_tracke_rs::{RetrievalError, StatusFilter, StatusKind, Task, TaskFilter, TaskTree};
use u4::U4 as u4;

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    List(ListArgs),
    Add(AddArgs),
    Start(StartArgs),
    Complete(CompleteArgs),
    Delete(DeleteArgs),
    Postpone(PostponeArgs),
    Purge,
}

#[derive(Args, Default)]
struct ListArgs {
    #[arg(conflicts_with_all = ["status", "all", "overdue", "due_within"])]
    id: Option<String>,

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
    #[arg(long)]
    due_within: Option<Duration>,
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
            parse_due(self.due_within)
        };

        TaskFilter { status, due_before }
    }
}

#[derive(Args)]
struct AddArgs {
    name: String,

    #[arg(long)]
    due: Option<Duration>,

    #[arg(long)]
    repeat: Option<Duration>,
}

#[derive(Args)]
struct StartArgs {
    id: String,
}

#[derive(Args)]
struct CompleteArgs {
    id: String,
}

#[derive(Args)]
struct DeleteArgs {
    id: String,
}

#[derive(Args)]
struct PostponeArgs {
    id: String,
    duration: Duration,
}

fn parse_due(duration: Option<humantime::Duration>) -> Option<DateTime<Local>> {
    duration.map(|d| {
        let due = std::time::SystemTime::now() + *d;
        DateTime::<Local>::from(due)
    })
}

fn parse_duration(duration: Option<humantime::Duration>) -> Option<chrono::Duration> {
    duration.and_then(|d| chrono::Duration::from_std(*d).ok())
}

#[allow(clippy::cast_possible_truncation)]
fn parse_hex_string(s: &str) -> Result<Vec<u4>> {
    s.chars()
        .map(|c| {
            c.to_digit(16)
                .map(|d| u4::new(d as u8).unwrap())
                .ok_or(RetrievalError::InvalidHexString)
                .map_err(Into::into)
        })
        .collect()
}

/// Saves the task tree to the provided path.
///
/// # Errors
///
/// This function will return an error if:
/// - The directory does not exist and cannot be created.
/// - Serialisation of the task tree fails.
pub fn save_task_tree<P: AsRef<Path>>(tree: &TaskTree, path: P) -> Result<()> {
    if let Some(parent) = path.as_ref().parent() {
        std::fs::create_dir_all(parent)?;
    }

    let file = File::create(path)?;
    let writer = BufWriter::new(file);

    serde_json::to_writer_pretty(writer, tree)?;

    Ok(())
}

/// Loads the task tree from the provided path.
///
/// # Errors
///
/// This function will return an error if:
/// - The path does not already exist.
/// - Deserialisation of the task tree fails.
pub fn load_task_tree<P: AsRef<Path>>(path: P) -> Result<TaskTree> {
    match File::open(&path) {
        Ok(file) => {
            let reader = BufReader::new(file);
            Ok(serde_json::from_reader(reader)?)
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let tree = TaskTree::new();
            Ok(tree)
        }
        Err(err) => Err(err.into()),
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let mut task_tree = load_task_tree(
        config_local_dir()
            .expect("Unable to determine local configuration directory.")
            .join("task-tracke-rs/tasks.json"),
    )?;

    let default_filter = ListArgs::default().filter();

    match &cli.command.unwrap_or(Commands::List(ListArgs::default())) {
        Commands::List(args) => match &args.id {
            Some(id) => print!("{}", task_tree.get_task(&parse_hex_string(id)?)?),
            None => print!("{}", task_tree.filtered(&args.filter())),
        },
        Commands::Add(args) => {
            task_tree.add_task(Task::new(
                args.name.clone(),
                parse_due(args.due),
                parse_duration(args.repeat),
            ));
            save_task_tree(
                &task_tree,
                config_local_dir()
                    .unwrap()
                    .join("task-tracke-rs/tasks.json"),
            )?;
            print!("{}", task_tree.filtered(&default_filter));
        }
        Commands::Start(args) => {
            task_tree
                .get_task_mut(&parse_hex_string(&args.id)?)?
                .start_task()?;
            save_task_tree(
                &task_tree,
                config_local_dir()
                    .unwrap()
                    .join("task-tracke-rs/tasks.json"),
            )?;
            print!("{}", task_tree.filtered(&default_filter));
        }
        Commands::Complete(args) => {
            if let Some(next) = task_tree
                .get_task_mut(&parse_hex_string(&args.id)?)?
                .complete_task()?
            {
                task_tree.add_task(next);
            }
            save_task_tree(
                &task_tree,
                config_local_dir()
                    .unwrap()
                    .join("task-tracke-rs/tasks.json"),
            )?;
            print!("{}", task_tree.filtered(&default_filter));
        }
        Commands::Delete(args) => {
            task_tree.delete_task(&parse_hex_string(&args.id)?)?;
            save_task_tree(
                &task_tree,
                config_local_dir()
                    .unwrap()
                    .join("task-tracke-rs/tasks.json"),
            )?;
            print!("{}", task_tree.filtered(&default_filter));
        }
        Commands::Postpone(args) => {
            task_tree
                .get_task_mut(&parse_hex_string(&args.id)?)?
                .postpone_task(parse_duration(Some(args.duration)).unwrap())?;
            save_task_tree(
                &task_tree,
                config_local_dir()
                    .unwrap()
                    .join("task-tracke-rs/tasks.json"),
            )?;
            print!("{}", task_tree.filtered(&default_filter));
        }
        Commands::Purge => {
            task_tree.purge()?;
            save_task_tree(
                &task_tree,
                config_local_dir()
                    .unwrap()
                    .join("task-tracke-rs/tasks.json"),
            )?;
            print!("{}", task_tree.filtered(&default_filter));
        }
    }

    Ok(())
}
