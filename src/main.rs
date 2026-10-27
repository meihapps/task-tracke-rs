use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Local};
use clap::{ArgGroup, Args, Parser, Subcommand};
use dirs::config_local_dir;
use std::{
    fs::File,
    io::{BufReader, BufWriter},
    path::Path,
};
use task_tracke_rs::{
    StatusFilter, StatusKind, Task, TaskEdit, TaskFilter, TaskId, TaskTree, parse_due_at,
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
    Purge,
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

    let path = config_local_dir()
        .context("Unable to determine local configuration directory.")?
        .join("task-tracke-rs/tasks.json");

    let mut task_tree = load_task_tree(&path)?;

    let default_filter = ListArgs::default().filter();

    match &cli.command.unwrap_or(Commands::List(ListArgs::default())) {
        Commands::List(args) => match &args.id {
            Some(id) => print!("{}", task_tree.get_task(id.as_ref())?),
            None => print!("{}", task_tree.filtered(&args.filter())),
        },
        Commands::Add(args) => {
            task_tree.add_task(Task::new(
                args.name.clone(),
                args.due_at.or(args.due),
                args.repeat,
            )?);
            save_task_tree(&task_tree, &path)?;
            print!("{}", task_tree.filtered(&default_filter));
        }
        Commands::Edit(args) => {
            task_tree
                .get_task_mut(args.id.as_ref())?
                .edit(args.edit())?;
            save_task_tree(&task_tree, &path)?;
            print!("{}", task_tree.filtered(&default_filter));
        }
        Commands::Start(args) => {
            task_tree.get_task_mut(args.id.as_ref())?.start_task()?;
            save_task_tree(&task_tree, &path)?;
            print!("{}", task_tree.filtered(&default_filter));
        }
        Commands::Complete(args) => {
            if let Some(next) = task_tree.get_task_mut(args.id.as_ref())?.complete_task()? {
                task_tree.add_task(next);
            }
            save_task_tree(&task_tree, &path)?;
            print!("{}", task_tree.filtered(&default_filter));
        }
        Commands::Delete(args) => {
            task_tree.delete_task(args.id.as_ref())?;
            save_task_tree(&task_tree, &path)?;
            print!("{}", task_tree.filtered(&default_filter));
        }
        Commands::Postpone(args) => {
            task_tree
                .get_task_mut(args.id.as_ref())?
                .postpone_task(args.duration)?;
            save_task_tree(&task_tree, &path)?;
            print!("{}", task_tree.filtered(&default_filter));
        }
        Commands::Purge => {
            task_tree.purge()?;
            save_task_tree(&task_tree, &path)?;
            print!("{}", task_tree.filtered(&default_filter));
        }
    }

    Ok(())
}
