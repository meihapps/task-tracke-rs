//! The task tree behind the `todo` command-line task tracker.
//!
//! Tasks live in a [`TaskTree`], addressed by short hexadecimal [`TaskId`]s. Every change to
//! the tree is an [`Effect`], and applying an effect returns the effect that reverses it.
//! Commands such as [`TaskTree::add_task`] return a [`Record`] of those inverses, which
//! [`TaskTree::apply_record`] uses to undo the command, returning a record that redoes it.

use chrono::{DateTime, Duration, Local, NaiveDate, NaiveDateTime, TimeZone};
use rand::random_range;
use serde::{Deserialize, Deserializer, Serialize};
use std::{
    cmp::max,
    fmt::{self, Write},
    str::FromStr,
    string::String,
};
use thiserror::Error;
use u4::U4;

#[derive(Serialize, Deserialize)]
pub struct TaskTree {
    children: [Option<TaskTreeNode>; 16],
}

#[derive(Serialize, Deserialize)]
pub enum TaskTreeNode {
    Tree(Box<TaskTree>),
    Task(Task),
}

#[derive(Serialize, Deserialize)]
pub struct Task {
    pub creation_date: DateTime<Local>,
    pub due: Option<DateTime<Local>>,
    pub name: String,
    pub status: TaskStatus,
    pub repeat: Option<Duration>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct TaskId(Vec<U4>);

/// A reversible change to a task tree. Applying an effect returns the effect that reverses it.
#[derive(Serialize, Deserialize)]
pub enum Effect {
    /// Inserts a task. If `displaced` is set, the node at the task's parent position is first
    /// moved down into a new subtree at that digit, making room for the task beside it.
    Insert {
        id: TaskId,
        task: Task,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        displaced: Option<U4>,
    },
    /// Removes a task. If this leaves its parent subtree with a single child, that child
    /// moves up to take the subtree's place.
    Remove {
        id: TaskId,
    },
    Start {
        id: TaskId,
        at: DateTime<Local>,
    },
    Unstart {
        id: TaskId,
    },
    Complete {
        id: TaskId,
        at: DateTime<Local>,
    },
    Uncomplete {
        id: TaskId,
    },
    Edit {
        id: TaskId,
        edit: TaskEdit,
    },
}

/// The inverses of the steps a command took, in the order the steps happened.
/// Applying a record reverses those steps, last first.
#[derive(Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Record(Vec<Effect>);

#[derive(Debug, Serialize, Deserialize, Ord, PartialOrd, Eq, PartialEq)]
pub enum TaskStatus {
    InProgress(DateTime<Local>),
    Incomplete,
    Complete(Option<DateTime<Local>>, DateTime<Local>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    Incomplete,
    InProgress,
    Complete,
}

#[derive(Clone, Copy)]
pub enum StatusFilter {
    Is(StatusKind),
    Not(StatusKind),
}

/// A set of changes to apply to a task. Fields left as `None` are unchanged.
#[derive(Default, Serialize, Deserialize)]
pub struct TaskEdit {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `Some(None)` clears the due date.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_some"
    )]
    pub due: Option<Option<DateTime<Local>>>,
    /// `Some(None)` clears the repeat.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_some"
    )]
    pub repeat: Option<Option<Duration>>,
}

/// Deserialises a present field as `Some`, so that a `null` value becomes `Some(None)` rather
/// than `None`. Absent fields fall back to `None` through `#[serde(default)]`.
fn deserialize_some<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    T::deserialize(deserializer).map(Some)
}

#[derive(Default)]
pub struct TaskFilter {
    pub status: Option<StatusFilter>,
    pub due_before: Option<DateTime<Local>>,
}

pub struct FilteredTaskTree<'a> {
    tree: &'a TaskTree,
    filter: &'a TaskFilter,
}

#[derive(Debug, Error)]
pub enum StatusError {
    #[error("Task already started.")]
    TaskAlreadyStarted,
    #[error("Task already completed.")]
    TaskAlreadyCompleted,
    #[error("Attempted to postpone a task with no due date.")]
    PostponedTaskWithNoDueDate,
    #[error("Task is not in progress.")]
    TaskNotInProgress,
    #[error("Task is not completed.")]
    TaskNotCompleted,
}

#[derive(Debug, Error)]
pub enum EffectError {
    #[error(transparent)]
    Retrieval(#[from] RetrievalError),
    #[error(transparent)]
    Status(#[from] StatusError),
    #[error(transparent)]
    InvalidTask(#[from] InvalidTaskError),
    #[error("Cannot insert a task at '{0}'.")]
    InvalidPosition(TaskId),
}

#[derive(Debug, Error)]
pub enum InvalidTaskError {
    #[error("A repeating task must have a due date.")]
    RepeatWithoutDueDate,
    #[error(transparent)]
    EmptyName(#[from] EmptyNameError),
}

#[derive(Debug, Error)]
#[error("Task name cannot be empty.")]
pub struct EmptyNameError;

#[derive(Debug, Error)]
pub enum RetrievalError {
    #[error("No task with id '{0}'.")]
    NoSuchId(TaskId),
}

#[derive(Debug, Error)]
#[error("Invalid task id '{0}'. Expected one or more hexadecimal digits, e.g. '3a'.")]
pub struct ParseTaskIdError(String);

#[derive(Debug, Error)]
#[error(
    "Invalid status filter '{0}'. Expected incomplete, in-progress or complete, optionally prefixed with '!'."
)]
pub struct ParseStatusFilterError(String);

#[derive(Debug, Error)]
pub enum ParseDueAtError {
    #[error("Invalid date '{0}'. Expected 'YYYY-MM-DD' or 'YYYY-MM-DD HH:MM'.")]
    InvalidFormat(String),
    #[error("'{0}' does not exist in the local timezone.")]
    NonexistentLocalTime(String),
}

#[derive(Debug, Error)]
pub enum ParseTimeDeltaError {
    #[error("Invalid duration '{0}' ({1}). Try e.g. '30m', '2h' or '1w2d'.")]
    Invalid(String, humantime::DurationError),
    #[error("Duration '{0}' is too large.")]
    TooLarge(String),
}

/// Parses a human-readable duration such as `1w` or `2h 30m`.
///
/// # Errors
///
/// This function will return an error if:
/// - The input is not a valid duration.
/// - The duration is too large to represent.
pub fn parse_time_delta(s: &str) -> Result<Duration, ParseTimeDeltaError> {
    let duration =
        humantime::parse_duration(s).map_err(|e| ParseTimeDeltaError::Invalid(s.to_owned(), e))?;
    Duration::from_std(duration).map_err(|_| ParseTimeDeltaError::TooLarge(s.to_owned()))
}

/// Formats a duration in words, largest units first, e.g. `1 week 2 days`.
/// Months and years are as long as [`parse_time_delta`] takes them to be. Anything below a
/// second is dropped.
#[must_use]
pub fn format_time_delta(duration: Duration) -> String {
    const UNITS: [(&str, u64); 7] = [
        ("year", 31_557_600),
        ("month", 2_630_016),
        ("week", 604_800),
        ("day", 86_400),
        ("hour", 3_600),
        ("minute", 60),
        ("second", 1),
    ];

    let mut remaining = duration.num_seconds().unsigned_abs();
    let mut parts = Vec::new();

    for (unit, seconds) in UNITS {
        let count = remaining / seconds;
        remaining %= seconds;
        if count > 0 {
            let plural = if count == 1 { "" } else { "s" };
            parts.push(format!("{count} {unit}{plural}"));
        }
    }

    if parts.is_empty() {
        "0 seconds".to_owned()
    } else {
        parts.join(" ")
    }
}

/// Parses a human-readable duration and returns the time that far from now.
///
/// # Errors
///
/// This function will return an error if:
/// - The input is not a valid duration.
/// - The resulting time is too far in the future to represent.
pub fn parse_due_in(s: &str) -> Result<DateTime<Local>, ParseTimeDeltaError> {
    Local::now()
        .checked_add_signed(parse_time_delta(s)?)
        .ok_or_else(|| ParseTimeDeltaError::TooLarge(s.to_owned()))
}

/// Parses a task name, rejecting names that are empty or only whitespace.
///
/// # Errors
///
/// This function will return an error if:
/// - The name is empty or only whitespace.
pub fn parse_name(s: &str) -> Result<String, EmptyNameError> {
    validate_name(s)?;
    Ok(s.to_owned())
}

/// Checks that a task name is not empty or only whitespace.
///
/// # Errors
///
/// This function will return an error if:
/// - The name is empty or only whitespace.
pub fn validate_name(s: &str) -> Result<(), EmptyNameError> {
    if s.trim().is_empty() {
        Err(EmptyNameError)
    } else {
        Ok(())
    }
}

/// Parses an absolute local date and time, as `YYYY-MM-DD HH:MM` or `YYYY-MM-DD` (midnight).
/// Times that occur twice due to a daylight saving change resolve to the earlier one.
///
/// # Errors
///
/// This function will return an error if:
/// - The input matches neither format.
/// - The time is skipped in the local timezone due to a daylight saving change.
pub fn parse_due_at(s: &str) -> Result<DateTime<Local>, ParseDueAtError> {
    let naive = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M")
        .or_else(|_| {
            NaiveDate::parse_from_str(s, "%Y-%m-%d").map(|d| d.and_time(chrono::NaiveTime::MIN))
        })
        .map_err(|_| ParseDueAtError::InvalidFormat(s.to_owned()))?;

    Local
        .from_local_datetime(&naive)
        .earliest()
        .ok_or_else(|| ParseDueAtError::NonexistentLocalTime(s.to_owned()))
}

impl TaskTree {
    #[must_use]
    pub fn new() -> TaskTree {
        TaskTree {
            children: std::array::from_fn(|_| Option::default()),
        }
    }

    /// Adds a task at a random free position.
    pub fn add_task(&mut self, task: Task) -> Record {
        Record(vec![self.insert_random(task)])
    }

    /// Deletes the task associated with the provided id.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The provided ID does not exist.
    pub fn delete_task(&mut self, id: &[U4]) -> Result<Record, EffectError> {
        let inverse = self.apply(Effect::Remove { id: id.into() })?;
        Ok(Record(vec![inverse]))
    }

    /// Starts the task associated with the provided id.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The provided ID does not exist.
    /// - The task is already in progress or completed.
    pub fn start_task(&mut self, id: &[U4]) -> Result<Record, EffectError> {
        let inverse = self.apply(Effect::Start {
            id: id.into(),
            at: Local::now(),
        })?;
        Ok(Record(vec![inverse]))
    }

    /// Completes the task associated with the provided id.
    /// Adds the task's next occurrence if it is recurring.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The provided ID does not exist.
    /// - The task is already completed.
    pub fn complete_task(&mut self, id: &[U4]) -> Result<Record, EffectError> {
        let next = self.get_task(id)?.next_occurrence();
        let mut record = Record(vec![self.apply(Effect::Complete {
            id: id.into(),
            at: Local::now(),
        })?]);

        if let Some(next) = next {
            record.0.push(self.insert_random(next));
        }

        Ok(record)
    }

    /// Postpones the task associated with the provided id. See [`Task::postponed_due`].
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The provided ID does not exist.
    /// - The task has no due date.
    pub fn postpone_task(&mut self, id: &[U4], duration: Duration) -> Result<Record, EffectError> {
        let due = self.get_task(id)?.postponed_due(duration)?;
        self.edit_task(
            id,
            TaskEdit {
                due: Some(Some(due)),
                ..TaskEdit::default()
            },
        )
    }

    /// Applies the given changes to the task associated with the provided id.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The provided ID does not exist.
    /// - The edit is rejected. See [`Task::edit`].
    pub fn edit_task(&mut self, id: &[U4], edit: TaskEdit) -> Result<Record, EffectError> {
        let inverse = self.apply(Effect::Edit {
            id: id.into(),
            edit,
        })?;
        Ok(Record(vec![inverse]))
    }

    /// Applies a record, returning the record that reverses it.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - Any effect is rejected. Effects applied before it are not rolled back.
    pub fn apply_record(&mut self, record: Record) -> Result<Record, EffectError> {
        // Each effect applied here is a step of its own, so the inverses are already in step order.
        record
            .0
            .into_iter()
            .rev()
            .map(|effect| self.apply(effect))
            .collect::<Result<_, _>>()
            .map(Record)
    }

    /// Applies an effect, returning the effect that reverses it.
    /// The tree is left unchanged if the effect is rejected.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The effect refers to a task that does not exist.
    /// - The effect inserts at a position that is occupied or unreachable.
    /// - The task is not in the status the effect expects.
    /// - The edit is rejected. See [`Task::edit`].
    pub fn apply(&mut self, effect: Effect) -> Result<Effect, EffectError> {
        Ok(match effect {
            Effect::Insert {
                id,
                task,
                displaced,
            } => {
                if self.insert(id.as_ref(), task, displaced).is_err() {
                    return Err(EffectError::InvalidPosition(id));
                }
                Effect::Remove { id }
            }
            Effect::Remove { id } => {
                let (task, displaced) = self
                    .remove(id.as_ref())
                    .ok_or_else(|| RetrievalError::no_such_id(id.as_ref()))?;
                Effect::Insert {
                    id,
                    task,
                    displaced,
                }
            }
            Effect::Start { id, at } => {
                self.get_task_mut(id.as_ref())?.start(at)?;
                Effect::Unstart { id }
            }
            Effect::Unstart { id } => {
                let at = self.get_task_mut(id.as_ref())?.unstart()?;
                Effect::Start { id, at }
            }
            Effect::Complete { id, at } => {
                self.get_task_mut(id.as_ref())?.complete(at)?;
                Effect::Uncomplete { id }
            }
            Effect::Uncomplete { id } => {
                let at = self.get_task_mut(id.as_ref())?.uncomplete()?;
                Effect::Complete { id, at }
            }
            Effect::Edit { id, edit } => {
                let edit = self.get_task_mut(id.as_ref())?.edit(edit)?;
                Effect::Edit { id, edit }
            }
        })
    }

    fn insert_random(&mut self, task: Task) -> Effect {
        let (id, displaced) = self.random_free_position();
        self.apply(Effect::Insert {
            id,
            task,
            displaced,
        })
        .unwrap_or_else(|_| unreachable!("Position was just chosen as free."))
    }

    /// Picks a random position for a new task. If it lands on an existing task, that task is
    /// displaced to one digit of a new subtree and the new task takes a different one, so
    /// every subtree keeps at least two children.
    fn random_free_position(&self) -> (TaskId, Option<U4>) {
        let mut id = TaskId::new();
        let mut tree = self;

        loop {
            let index = random_range(0..16);
            id.push(digit(index));

            match &tree.children[index] {
                None => return (id, None),
                Some(TaskTreeNode::Tree(child)) => tree = child,
                Some(TaskTreeNode::Task(_)) => {
                    let displaced = random_range(0..16);
                    id.push(digit((displaced + random_range(1..16)) % 16));
                    return (id, Some(digit(displaced)));
                }
            }
        }
    }

    /// Inserts a task at the given position, as described by [`Effect::Insert`].
    /// Returns the task if the position is occupied or unreachable.
    fn insert(&mut self, id: &[U4], task: Task, displaced: Option<U4>) -> Result<(), Task> {
        let Some((&index, rest)) = id.split_first() else {
            return Err(task);
        };
        let slot = &mut self.children[usize::from(index)];

        match (rest, displaced) {
            ([], None) if slot.is_none() => {
                *slot = Some(TaskTreeNode::Task(task));
                Ok(())
            }
            ([last], Some(displaced)) if slot.is_some() && *last != displaced => {
                let mut tree = TaskTree::new();
                tree.children[usize::from(displaced)] = slot.take();
                tree.children[usize::from(*last)] = Some(TaskTreeNode::Task(task));
                *slot = Some(TaskTreeNode::Tree(Box::new(tree)));
                Ok(())
            }
            _ => match slot {
                Some(TaskTreeNode::Tree(tree)) if !rest.is_empty() => {
                    tree.insert(rest, task, displaced)
                }
                _ => Err(task),
            },
        }
    }

    /// Removes the task at the given position, as described by [`Effect::Remove`].
    /// Returns the task, and the old digit of the child that moved up, if any.
    fn remove(&mut self, id: &[U4]) -> Option<(Task, Option<U4>)> {
        let (&index, rest) = id.split_first()?;
        let slot = &mut self.children[usize::from(index)];

        match slot {
            Some(TaskTreeNode::Task(_)) if rest.is_empty() => {
                let Some(TaskTreeNode::Task(task)) = slot.take() else {
                    unreachable!("Slot was just matched as a task.");
                };
                Some((task, None))
            }
            Some(TaskTreeNode::Tree(tree)) if !rest.is_empty() => {
                let (task, displaced) = tree.remove(rest)?;
                // Only the removed task's direct parent can be left with a single child.
                if rest.len() > 1 {
                    return Some((task, displaced));
                }

                let mut remaining = tree
                    .children
                    .iter()
                    .enumerate()
                    .filter_map(|(i, child)| child.as_ref().map(|_| i));
                let (Some(only), None) = (remaining.next(), remaining.next()) else {
                    return Some((task, None));
                };

                let child = tree.children[only].take();
                *slot = child;
                Some((task, Some(digit(only))))
            }
            _ => None,
        }
    }

    /// Collapses every subtree with fewer than two children. Trees built through [`Effect`]s
    /// are always in this form, so this is only needed for trees saved by older versions.
    pub fn optimise(&mut self) {
        for i in 0..self.children.len() {
            let Some(TaskTreeNode::Tree(tree)) = self.children[i].as_mut() else {
                continue;
            };

            tree.optimise();

            let mut remaining = tree
                .children
                .iter()
                .enumerate()
                .filter_map(|(i, child)| child.as_ref().map(|_| i));

            let replacement = match (remaining.next(), remaining.next()) {
                (None, _) => None,
                (Some(i), None) => tree.children[i].take(),
                (Some(_), Some(_)) => continue,
            };

            self.children[i] = replacement;
        }
    }

    /// Gets a task from the task tree by id.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - There is no task with the provided ID.
    pub fn get_task(&self, id: &[U4]) -> Result<&Task, RetrievalError> {
        self.find(id).ok_or_else(|| RetrievalError::no_such_id(id))
    }

    fn find(&self, id: &[U4]) -> Option<&Task> {
        let (&index, rest) = id.split_first()?;

        match (self.children[usize::from(index)].as_ref()?, rest.is_empty()) {
            (TaskTreeNode::Task(task), true) => Some(task),
            (TaskTreeNode::Tree(tree), false) => tree.find(rest),
            _ => None,
        }
    }

    /// Gets a task from the task tree by id.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - There is no task with the provided ID.
    pub fn get_task_mut(&mut self, id: &[U4]) -> Result<&mut Task, RetrievalError> {
        self.find_mut(id)
            .ok_or_else(|| RetrievalError::no_such_id(id))
    }

    fn find_mut(&mut self, id: &[U4]) -> Option<&mut Task> {
        let (&index, rest) = id.split_first()?;

        match (self.children[usize::from(index)].as_mut()?, rest.is_empty()) {
            (TaskTreeNode::Task(task), true) => Some(task),
            (TaskTreeNode::Tree(tree), false) => tree.find_mut(rest),
            _ => None,
        }
    }

    fn max_id_len(&self) -> usize {
        self.children.iter().flatten().fold(0, |acc, child| {
            acc.max(match child {
                TaskTreeNode::Task(_) => 1,
                TaskTreeNode::Tree(tree) => 1 + tree.max_id_len(),
            })
        })
    }

    fn fmt_helper(
        &self,
        f: &mut fmt::Formatter<'_>,
        prefix: &TaskId,
        id_width: usize,
        filter: &TaskFilter,
    ) -> fmt::Result {
        let mut tasks = self.collect_tasks(prefix);
        tasks.retain(|(_, task)| filter.matches(task));

        tasks.sort_by(|a, b| {
            a.1.status
                .cmp(&b.1.status)
                // Soonest due first, tasks without a due date last.
                .then_with(|| a.1.due.is_none().cmp(&b.1.due.is_none()))
                .then_with(|| a.1.due.cmp(&b.1.due))
        });

        let name_width = tasks
            .iter()
            .map(|(_, task)| task.name.len())
            .max()
            .unwrap_or(0);

        let info: Vec<String> = tasks
            .iter()
            .map(|(_, task)| match &task.status {
                TaskStatus::Incomplete => task
                    .due
                    .map(|d| format!("due: {}", d.format("%Y-%m-%d %H:%M")))
                    .unwrap_or_default(),

                TaskStatus::InProgress(started) => {
                    format!("started: {}", started.format("%Y-%m-%d %H:%M"))
                }

                TaskStatus::Complete(_, completed) => {
                    format!("completed: {}", completed.format("%Y-%m-%d %H:%M"))
                }
            })
            .collect();

        let info_width = info.iter().map(String::len).max().unwrap_or(0);

        for ((id, task), info) in tasks.into_iter().zip(info) {
            let marker = match task.status {
                TaskStatus::Incomplete => "",
                TaskStatus::InProgress(_) => "-",
                TaskStatus::Complete(_, _) => "X",
            };
            let name = &task.name;
            writeln!(
                f,
                "{id:<id_width$} {marker:<2} {name:<name_width$} {info:<info_width$}",
            )?;
        }

        Ok(())
    }

    #[must_use]
    pub fn filtered<'a>(&'a self, filter: &'a TaskFilter) -> FilteredTaskTree<'a> {
        FilteredTaskTree { tree: self, filter }
    }

    #[must_use]
    pub fn collect_tasks(&self, prefix: &TaskId) -> Vec<(TaskId, &Task)> {
        let mut tasks = Vec::new();

        for (i, child) in self.children.iter().enumerate() {
            let Some(child) = child else {
                continue;
            };

            let mut id = prefix.clone();
            id.push(digit(i));

            match child {
                TaskTreeNode::Task(task) => tasks.push((id, task)),
                TaskTreeNode::Tree(tree) => tasks.extend(tree.collect_tasks(&id)),
            }
        }

        tasks
    }
}

impl Default for TaskTree {
    fn default() -> Self {
        Self::new()
    }
}

impl Task {
    /// Creates a new incomplete task.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The name is empty or only whitespace.
    /// - The task repeats without a due date.
    pub fn new(
        name: String,
        due_date: Option<DateTime<Local>>,
        recurrence: Option<Duration>,
    ) -> Result<Task, InvalidTaskError> {
        validate_name(&name)?;

        if recurrence.is_some() && due_date.is_none() {
            return Err(InvalidTaskError::RepeatWithoutDueDate);
        }

        Ok(Task {
            creation_date: Local::now(),
            due: due_date,
            name,
            status: TaskStatus::Incomplete,
            repeat: recurrence,
        })
    }

    /// Sets `status` to `InProgress`.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The task is already in progress.
    /// - The task is already completed.
    fn start(&mut self, at: DateTime<Local>) -> Result<(), StatusError> {
        match self.status {
            TaskStatus::Incomplete => {
                self.status = TaskStatus::InProgress(at);
                Ok(())
            }
            TaskStatus::InProgress(_) => Err(StatusError::TaskAlreadyStarted),
            TaskStatus::Complete(_, _) => Err(StatusError::TaskAlreadyCompleted),
        }
    }

    /// Sets `status` from `InProgress` back to `Incomplete`. Returns when the task was started.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The task is not in progress.
    fn unstart(&mut self) -> Result<DateTime<Local>, StatusError> {
        match self.status {
            TaskStatus::InProgress(at) => {
                self.status = TaskStatus::Incomplete;
                Ok(at)
            }
            _ => Err(StatusError::TaskNotInProgress),
        }
    }

    /// Sets `status` to `Complete`.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The task is already completed.
    fn complete(&mut self, at: DateTime<Local>) -> Result<(), StatusError> {
        self.status = match self.status {
            TaskStatus::Incomplete => TaskStatus::Complete(None, at),
            TaskStatus::InProgress(start_time) => TaskStatus::Complete(Some(start_time), at),
            TaskStatus::Complete(_, _) => return Err(StatusError::TaskAlreadyCompleted),
        };
        Ok(())
    }

    /// Restores `status` to what it was before completion. Returns when the task was completed.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The task is not completed.
    fn uncomplete(&mut self) -> Result<DateTime<Local>, StatusError> {
        match self.status {
            TaskStatus::Complete(start_time, at) => {
                self.status = start_time.map_or(TaskStatus::Incomplete, TaskStatus::InProgress);
                Ok(at)
            }
            _ => Err(StatusError::TaskNotCompleted),
        }
    }

    /// Returns the next occurrence of a recurring task, or `None` if it does not repeat.
    #[must_use]
    pub fn next_occurrence(&self) -> Option<Task> {
        let (Some(recurrence), Some(due_date)) = (self.repeat, self.due) else {
            return None;
        };

        // Built directly rather than via `Task::new`, so that tasks saved before
        // validation existed can still be completed.
        Some(Task {
            creation_date: Local::now(),
            due: Some(due_date + recurrence),
            name: self.name.clone(),
            status: TaskStatus::Incomplete,
            repeat: self.repeat,
        })
    }

    /// Returns the due date after postponing by the given duration.
    /// If the due date is in the past, it will postpone relative to current time.
    /// If the due date is in the future, it will postpone relative to due date.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The task has no due date.
    pub fn postponed_due(&self, duration: Duration) -> Result<DateTime<Local>, StatusError> {
        self.due
            .map(|due| max(due, Local::now()) + duration)
            .ok_or(StatusError::PostponedTaskWithNoDueDate)
    }

    /// Applies the given changes to the task, returning the edit that reverses them.
    /// The task is left unchanged if the edit is rejected.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The edited task would repeat without a due date.
    /// - The new name is empty or only whitespace.
    pub fn edit(&mut self, edit: TaskEdit) -> Result<TaskEdit, InvalidTaskError> {
        if let Some(name) = &edit.name {
            validate_name(name)?;
        }
        let due = edit.due.unwrap_or(self.due);
        let repeat = edit.repeat.unwrap_or(self.repeat);

        if repeat.is_some() && due.is_none() {
            return Err(InvalidTaskError::RepeatWithoutDueDate);
        }

        Ok(TaskEdit {
            name: edit
                .name
                .map(|name| std::mem::replace(&mut self.name, name)),
            due: edit.due.map(|_| std::mem::replace(&mut self.due, due)),
            repeat: edit
                .repeat
                .map(|_| std::mem::replace(&mut self.repeat, repeat)),
        })
    }
}

impl TaskStatus {
    #[must_use]
    pub fn kind(&self) -> StatusKind {
        match self {
            TaskStatus::Incomplete => StatusKind::Incomplete,
            TaskStatus::InProgress(_) => StatusKind::InProgress,
            TaskStatus::Complete(_, _) => StatusKind::Complete,
        }
    }
}

impl StatusFilter {
    #[must_use]
    pub fn matches(self, kind: StatusKind) -> bool {
        match self {
            StatusFilter::Is(expected) => kind == expected,
            StatusFilter::Not(excluded) => kind != excluded,
        }
    }
}

impl FromStr for StatusFilter {
    type Err = ParseStatusFilterError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (negated, kind) = match s.strip_prefix('!') {
            Some(rest) => (true, rest),
            None => (false, s),
        };

        let kind = match kind {
            "incomplete" => StatusKind::Incomplete,
            "in-progress" => StatusKind::InProgress,
            "complete" => StatusKind::Complete,
            _ => return Err(ParseStatusFilterError(s.to_owned())),
        };

        Ok(if negated {
            StatusFilter::Not(kind)
        } else {
            StatusFilter::Is(kind)
        })
    }
}

impl TaskFilter {
    /// Returns whether the task passes every filter that is set.
    /// Tasks without a due date never pass a `due_before` filter.
    #[must_use]
    pub fn matches(&self, task: &Task) -> bool {
        self.status
            .is_none_or(|status| status.matches(task.status.kind()))
            && self
                .due_before
                .is_none_or(|before| task.due.is_some_and(|due| due <= before))
    }
}

impl TaskId {
    pub fn push(&mut self, digit: U4) {
        self.0.push(digit);
    }

    #[must_use]
    pub fn new() -> Self {
        Self(Vec::new())
    }
}

impl Default for TaskId {
    fn default() -> Self {
        Self::new()
    }
}

impl AsRef<[U4]> for TaskId {
    fn as_ref(&self) -> &[U4] {
        &self.0
    }
}

impl Clone for TaskId {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl FromStr for TaskId {
    type Err = ParseTaskIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err(ParseTaskIdError(s.to_owned()));
        }

        s.chars()
            .map(|c| c.to_digit(16).and_then(|d| U4::try_from(d).ok()))
            .collect::<Option<Vec<U4>>>()
            .map(Self)
            .ok_or_else(|| ParseTaskIdError(s.to_owned()))
    }
}

impl From<&[U4]> for TaskId {
    fn from(id: &[U4]) -> Self {
        Self(id.to_vec())
    }
}

impl From<TaskId> for String {
    fn from(id: TaskId) -> Self {
        id.to_string()
    }
}

impl TryFrom<String> for TaskId {
    type Error = ParseTaskIdError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

/// Converts a child index into an id digit.
fn digit(index: usize) -> U4 {
    U4::try_from(index).unwrap_or_else(|_| unreachable!("Child indices are below 16."))
}

impl RetrievalError {
    fn no_such_id(id: &[U4]) -> Self {
        RetrievalError::NoSuchId(TaskId(id.to_vec()))
    }
}

impl fmt::Display for TaskTree {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.filtered(&TaskFilter::default()).fmt(f)
    }
}

impl fmt::Display for FilteredTaskTree<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let id_width = self.tree.max_id_len().next_multiple_of(4);
        self.tree
            .fmt_helper(f, &TaskId::new(), id_width, self.filter)
    }
}

impl fmt::Display for Task {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}\n\nstatus:  {}\ncreated: {}\ndue:     {:}\nrepeat:  {}",
            self.name,
            self.status,
            self.creation_date,
            self.due.map_or("n/a".to_owned(), |x| x.to_rfc2822()),
            self.repeat.map_or("n/a".to_owned(), format_time_delta)
        )
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut s = String::with_capacity(self.0.len());
        for digit in &self.0 {
            write!(s, "{digit:x}")?;
        }
        f.pad(&s)
    }
}

impl fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                TaskStatus::InProgress(timestamp) => format!("In Progress Since {timestamp}"),
                TaskStatus::Incomplete => "Incomplete".to_string(),
                TaskStatus::Complete(start, end) => match start {
                    Some(start) => format!("Started {start}. Completed {end}"),
                    None => format!("Completed {end}"),
                },
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> TaskId {
        s.parse().unwrap()
    }

    fn u4(n: u8) -> U4 {
        U4::try_from(n).unwrap()
    }

    fn at(hour: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(2030, 1, 1, hour, 0, 0).unwrap()
    }

    fn task(name: &str) -> Task {
        Task::new(name.to_owned(), None, None).unwrap()
    }

    fn repeating(name: &str) -> Task {
        Task::new(name.to_owned(), Some(at(9)), Some(Duration::days(7))).unwrap()
    }

    /// The whole tree, including every task field, for exact comparisons.
    fn snapshot(tree: &TaskTree) -> String {
        serde_json::to_string(tree).unwrap()
    }

    /// Every task as `(id, name)`, sorted by id.
    fn layout(tree: &TaskTree) -> Vec<(String, String)> {
        let mut tasks: Vec<_> = tree
            .collect_tasks(&TaskId::new())
            .into_iter()
            .map(|(id, task)| (id.to_string(), task.name.clone()))
            .collect();
        tasks.sort();
        tasks
    }

    fn pairs(expected: &[(&str, &str)]) -> Vec<(String, String)> {
        expected
            .iter()
            .map(|&(id, name)| (id.to_owned(), name.to_owned()))
            .collect()
    }

    /// Asserts that every subtree below the root has at least two children.
    fn assert_compact(tree: &TaskTree, is_root: bool) {
        let children: Vec<_> = tree.children.iter().flatten().collect();
        assert!(
            is_root || children.len() >= 2,
            "subtree with fewer than two children"
        );
        for child in children {
            if let TaskTreeNode::Tree(subtree) = child {
                assert_compact(subtree, false);
            }
        }
    }

    fn insert(tree: &mut TaskTree, at: &str, name: &str, displaced: Option<u8>) -> Effect {
        tree.apply(Effect::Insert {
            id: id(at),
            task: task(name),
            displaced: displaced.map(u4),
        })
        .unwrap()
    }

    /// Builds `3: tree { 1: a, 2: b, 7: tree { 0: c, 5: d } }` and `9: e`.
    fn sample_tree() -> TaskTree {
        let mut tree = TaskTree::new();
        insert(&mut tree, "3", "a", None);
        insert(&mut tree, "32", "b", Some(1));
        insert(&mut tree, "37", "c", None);
        insert(&mut tree, "375", "d", Some(0));
        insert(&mut tree, "9", "e", None);
        tree
    }

    /// Applies an effect, then its inverse, then the inverse of that, checking each step
    /// lands back on the right state.
    fn assert_round_trip(tree: &mut TaskTree, effect: Effect) {
        let before = snapshot(tree);
        let inverse = tree.apply(effect).unwrap();
        let after = snapshot(tree);
        assert_ne!(before, after, "effect changed nothing");

        let redo = tree.apply(inverse).unwrap();
        assert_eq!(snapshot(tree), before, "inverse did not restore the tree");
        tree.apply(redo).unwrap();
        assert_eq!(snapshot(tree), after, "redo did not restore the change");
    }

    /// Applies a record, then its inverse, checking both directions.
    fn assert_record_round_trip(tree: &mut TaskTree, before: &str, record: Record) {
        let after = snapshot(tree);
        let redo = tree.apply_record(record).unwrap();
        assert_eq!(snapshot(tree), before, "undo did not restore the tree");
        let undo = tree.apply_record(redo).unwrap();
        assert_eq!(snapshot(tree), after, "redo did not restore the change");
        tree.apply_record(undo).unwrap();
        assert_eq!(
            snapshot(tree),
            before,
            "second undo did not restore the tree"
        );
    }

    // Parsing.

    #[test]
    fn task_id_parses_hex_digits() {
        assert_eq!(id("3aF").as_ref(), &[u4(3), u4(10), u4(15)]);
        assert_eq!(id("3aF").to_string(), "3af");
    }

    #[test]
    fn task_id_rejects_empty_and_non_hex() {
        assert!("".parse::<TaskId>().is_err());
        assert!("3g".parse::<TaskId>().is_err());
        assert!("-1".parse::<TaskId>().is_err());
    }

    #[test]
    fn task_id_display_respects_padding() {
        assert_eq!(format!("{:<4}|", id("3a")), "3a  |");
    }

    #[test]
    fn task_id_serialises_as_hex_string() {
        assert_eq!(serde_json::to_string(&id("3a")).unwrap(), r#""3a""#);
        let parsed: TaskId = serde_json::from_str(r#""3a""#).unwrap();
        assert_eq!(parsed.as_ref(), id("3a").as_ref());
        assert!(serde_json::from_str::<TaskId>(r#""zz""#).is_err());
    }

    #[test]
    fn parse_name_rejects_blank_names() {
        assert_eq!(parse_name(" x ").unwrap(), " x ");
        assert!(parse_name("").is_err());
        assert!(parse_name(" \t\n").is_err());
    }

    #[test]
    fn parse_due_at_accepts_date_and_date_time() {
        assert_eq!(
            parse_due_at("2030-01-01").unwrap(),
            Local.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap()
        );
        assert_eq!(parse_due_at("2030-01-01 09:00").unwrap(), at(9));
    }

    #[test]
    fn parse_due_at_rejects_other_formats() {
        for input in ["", "2030-13-01", "01/01/2030", "2030-01-01 9am"] {
            assert!(
                matches!(parse_due_at(input), Err(ParseDueAtError::InvalidFormat(_))),
                "{input:?} was accepted"
            );
        }
    }

    #[test]
    fn parse_time_delta_accepts_compound_durations() {
        assert_eq!(parse_time_delta("30m").unwrap(), Duration::minutes(30));
        assert_eq!(parse_time_delta("1w 2d").unwrap(), Duration::days(9));
        assert_eq!(parse_time_delta("1w2d").unwrap(), Duration::days(9));
        assert!(matches!(
            parse_time_delta("soon"),
            Err(ParseTimeDeltaError::Invalid(..))
        ));
    }

    #[test]
    fn format_time_delta_uses_spaced_words() {
        assert_eq!(format_time_delta(Duration::days(1)), "1 day");
        assert_eq!(format_time_delta(Duration::days(2)), "2 days");
        assert_eq!(format_time_delta(Duration::days(9)), "1 week 2 days");
        assert_eq!(
            format_time_delta(Duration::hours(25) + Duration::seconds(61)),
            "1 day 1 hour 1 minute 1 second"
        );
        assert_eq!(format_time_delta(Duration::milliseconds(500)), "0 seconds");
    }

    #[test]
    fn format_time_delta_matches_parsed_units() {
        for (input, expected) in [
            ("1d", "1 day"),
            ("1w", "1 week"),
            ("3w", "3 weeks"),
            ("1month", "1 month"),
            ("2months", "2 months"),
            ("1y", "1 year"),
            (
                "1y 1month 1w 1d 1h 1m 1s",
                "1 year 1 month 1 week 1 day 1 hour 1 minute 1 second",
            ),
        ] {
            assert_eq!(
                format_time_delta(parse_time_delta(input).unwrap()),
                expected
            );
        }
    }

    #[test]
    fn task_details_show_repeat_in_words() {
        let details = repeating("x").to_string();
        assert!(details.contains("repeat:  1 week"), "{details}");
        assert!(task("x").to_string().contains("repeat:  n/a"));
    }

    #[test]
    fn parse_due_in_is_relative_to_now() {
        let before = Local::now();
        let due = parse_due_in("1h").unwrap();
        assert!(due >= before + Duration::hours(1));
        assert!(due <= Local::now() + Duration::hours(1));
    }

    #[test]
    fn status_filter_parses_kinds_and_negation() {
        assert!(matches!(
            "complete".parse(),
            Ok(StatusFilter::Is(StatusKind::Complete))
        ));
        assert!(matches!(
            "!in-progress".parse(),
            Ok(StatusFilter::Not(StatusKind::InProgress))
        ));
        assert!("done".parse::<StatusFilter>().is_err());
        assert!("!!complete".parse::<StatusFilter>().is_err());
    }

    #[test]
    fn task_filter_combines_status_and_due() {
        let mut due_soon = Task::new("x".to_owned(), Some(at(9)), None).unwrap();
        let no_due = task("y");

        let filter = TaskFilter {
            status: Some(StatusFilter::Not(StatusKind::Complete)),
            due_before: Some(at(10)),
        };
        assert!(filter.matches(&due_soon));
        assert!(
            !filter.matches(&no_due),
            "tasks without a due date never match"
        );

        due_soon.complete(at(9)).unwrap();
        assert!(!filter.matches(&due_soon));
        assert!(TaskFilter::default().matches(&due_soon));
    }

    // Task state.

    #[test]
    fn new_task_validates_name_and_repeat() {
        assert!(matches!(
            Task::new(" ".to_owned(), None, None),
            Err(InvalidTaskError::EmptyName(_))
        ));
        assert!(matches!(
            Task::new("x".to_owned(), None, Some(Duration::days(1))),
            Err(InvalidTaskError::RepeatWithoutDueDate)
        ));
        assert_eq!(task("x").status, TaskStatus::Incomplete);
    }

    #[test]
    fn start_and_unstart_round_trip() {
        let mut task = task("x");
        task.start(at(9)).unwrap();
        assert_eq!(task.status, TaskStatus::InProgress(at(9)));
        assert!(matches!(
            task.start(at(10)),
            Err(StatusError::TaskAlreadyStarted)
        ));

        assert_eq!(task.unstart().unwrap(), at(9));
        assert_eq!(task.status, TaskStatus::Incomplete);
        assert!(matches!(
            task.unstart(),
            Err(StatusError::TaskNotInProgress)
        ));
    }

    #[test]
    fn complete_and_uncomplete_from_incomplete() {
        let mut task = task("x");
        task.complete(at(10)).unwrap();
        assert_eq!(task.status, TaskStatus::Complete(None, at(10)));
        assert!(matches!(
            task.complete(at(11)),
            Err(StatusError::TaskAlreadyCompleted)
        ));
        assert!(matches!(
            task.start(at(11)),
            Err(StatusError::TaskAlreadyCompleted)
        ));

        assert_eq!(task.uncomplete().unwrap(), at(10));
        assert_eq!(task.status, TaskStatus::Incomplete);
        assert!(matches!(
            task.uncomplete(),
            Err(StatusError::TaskNotCompleted)
        ));
    }

    #[test]
    fn complete_and_uncomplete_from_in_progress() {
        let mut task = task("x");
        task.start(at(9)).unwrap();
        task.complete(at(10)).unwrap();
        assert_eq!(task.status, TaskStatus::Complete(Some(at(9)), at(10)));

        task.uncomplete().unwrap();
        assert_eq!(task.status, TaskStatus::InProgress(at(9)));
    }

    #[test]
    fn next_occurrence_only_for_repeating_tasks() {
        assert!(task("x").next_occurrence().is_none());

        let next = repeating("x").next_occurrence().unwrap();
        assert_eq!(next.name, "x");
        assert_eq!(next.due, Some(at(9) + Duration::days(7)));
        assert_eq!(next.repeat, Some(Duration::days(7)));
        assert_eq!(next.status, TaskStatus::Incomplete);
    }

    #[test]
    fn postponed_due_is_relative_to_later_of_due_and_now() {
        let future = Local::now() + Duration::days(30);
        let task = Task::new("x".to_owned(), Some(future), None).unwrap();
        assert_eq!(
            task.postponed_due(Duration::hours(2)).unwrap(),
            future + Duration::hours(2)
        );

        let past = Local::now() - Duration::days(30);
        let task = Task::new("x".to_owned(), Some(past), None).unwrap();
        let before = Local::now();
        let due = task.postponed_due(Duration::hours(2)).unwrap();
        assert!(due >= before + Duration::hours(2));

        assert!(matches!(
            self::task("x").postponed_due(Duration::hours(2)),
            Err(StatusError::PostponedTaskWithNoDueDate)
        ));
    }

    #[test]
    fn edit_returns_inverse_of_changed_fields_only() {
        let mut task = repeating("old");
        let inverse = task
            .edit(TaskEdit {
                name: Some("new".to_owned()),
                due: Some(Some(at(12))),
                repeat: None,
            })
            .unwrap();

        assert_eq!(task.name, "new");
        assert_eq!(task.due, Some(at(12)));
        assert_eq!(inverse.name.as_deref(), Some("old"));
        assert_eq!(inverse.due, Some(Some(at(9))));
        assert!(inverse.repeat.is_none());

        task.edit(inverse).unwrap();
        assert_eq!(task.name, "old");
        assert_eq!(task.due, Some(at(9)));
        assert_eq!(task.repeat, Some(Duration::days(7)));
    }

    #[test]
    fn edit_can_clear_fields_and_restore_them() {
        let mut task = repeating("x");
        let inverse = task
            .edit(TaskEdit {
                due: Some(None),
                repeat: Some(None),
                ..TaskEdit::default()
            })
            .unwrap();
        assert_eq!((task.due, task.repeat), (None, None));

        task.edit(inverse).unwrap();
        assert_eq!(task.due, Some(at(9)));
        assert_eq!(task.repeat, Some(Duration::days(7)));
    }

    #[test]
    fn rejected_edit_leaves_task_unchanged() {
        let mut task = repeating("x");
        let clear_due = TaskEdit {
            name: Some("y".to_owned()),
            due: Some(None),
            ..TaskEdit::default()
        };
        assert!(matches!(
            task.edit(clear_due),
            Err(InvalidTaskError::RepeatWithoutDueDate)
        ));
        let blank_name = TaskEdit {
            name: Some(" ".to_owned()),
            due: Some(Some(at(12))),
            ..TaskEdit::default()
        };
        assert!(matches!(
            task.edit(blank_name),
            Err(InvalidTaskError::EmptyName(_))
        ));

        assert_eq!(task.name, "x");
        assert_eq!(task.due, Some(at(9)));
    }

    #[test]
    fn task_edit_serialisation_distinguishes_unset_and_cleared() {
        let edit = TaskEdit {
            name: None,
            due: Some(None),
            repeat: Some(Some(Duration::days(1))),
        };
        let json = serde_json::to_string(&edit).unwrap();
        assert!(!json.contains("name"), "unset fields are omitted: {json}");

        let parsed: TaskEdit = serde_json::from_str(&json).unwrap();
        assert!(parsed.name.is_none());
        assert_eq!(parsed.due, Some(None));
        assert_eq!(parsed.repeat, Some(Some(Duration::days(1))));
    }

    // Tree structure.

    #[test]
    fn sample_tree_has_expected_layout() {
        let tree = sample_tree();
        assert_eq!(
            layout(&tree),
            pairs(&[
                ("31", "a"),
                ("32", "b"),
                ("370", "c"),
                ("375", "d"),
                ("9", "e")
            ])
        );
        assert_compact(&tree, true);
    }

    #[test]
    fn insert_into_empty_slot() {
        let mut tree = TaskTree::new();
        let inverse = insert(&mut tree, "4", "a", None);
        assert!(matches!(inverse, Effect::Remove { id } if id.to_string() == "4"));
        assert_eq!(layout(&tree), pairs(&[("4", "a")]));
    }

    #[test]
    fn insert_with_displacement_moves_existing_task_down() {
        let mut tree = TaskTree::new();
        insert(&mut tree, "4", "a", None);
        insert(&mut tree, "4b", "b", Some(2));
        assert_eq!(layout(&tree), pairs(&[("42", "a"), ("4b", "b")]));
    }

    #[test]
    fn insert_with_displacement_moves_existing_subtree_down() {
        let mut tree = sample_tree();
        insert(&mut tree, "3f", "f", Some(6));
        assert_eq!(
            layout(&tree),
            pairs(&[
                ("361", "a"),
                ("362", "b"),
                ("3670", "c"),
                ("3675", "d"),
                ("3f", "f"),
                ("9", "e"),
            ])
        );
        assert_compact(&tree, true);
    }

    #[test]
    fn invalid_inserts_are_rejected_without_changes() {
        let mut tree = sample_tree();
        let before = snapshot(&tree);

        for (at, displaced) in [
            ("9", None),       // Occupied.
            ("31", None),      // Occupied, inside a subtree.
            ("91", None),      // Path runs through a task.
            ("5", Some(1)),    // Nothing to displace at the root.
            ("51", Some(2)),   // Nothing to displace.
            ("91", Some(1)),   // Displaced digit collides with the new task.
            ("", None),        // Empty id.
            ("3", None),       // Occupied by a subtree.
            ("9123", Some(1)), // Path runs through a task.
        ] {
            let result = tree.apply(Effect::Insert {
                id: id_or_empty(at),
                task: task("x"),
                displaced: displaced.map(u4),
            });
            assert!(
                matches!(result, Err(EffectError::InvalidPosition(_))),
                "insert at {at:?} displacing {displaced:?} was accepted"
            );
        }

        assert_eq!(snapshot(&tree), before);
    }

    fn id_or_empty(s: &str) -> TaskId {
        if s.is_empty() { TaskId::new() } else { id(s) }
    }

    #[test]
    fn remove_with_remaining_siblings_moves_nothing() {
        let mut tree = sample_tree();
        let inverse = tree.apply(Effect::Remove { id: id("31") }).unwrap();
        assert!(matches!(
            inverse,
            Effect::Insert { ref id, ref task, displaced: None }
                if id.to_string() == "31" && task.name == "a"
        ));
        assert_eq!(
            layout(&tree),
            pairs(&[("32", "b"), ("370", "c"), ("375", "d"), ("9", "e")])
        );
    }

    #[test]
    fn remove_leaving_one_task_moves_it_up() {
        let mut tree = sample_tree();
        let inverse = tree.apply(Effect::Remove { id: id("370") }).unwrap();
        assert!(matches!(inverse, Effect::Insert { displaced: Some(d), .. } if d == u4(5)));
        assert_eq!(
            layout(&tree),
            pairs(&[("31", "a"), ("32", "b"), ("37", "d"), ("9", "e")])
        );
        assert_compact(&tree, true);
    }

    #[test]
    fn remove_leaving_one_subtree_moves_it_up() {
        let mut tree = TaskTree::new();
        insert(&mut tree, "3", "a", None);
        insert(&mut tree, "37", "c", Some(1));
        insert(&mut tree, "375", "d", Some(0));
        assert_eq!(
            layout(&tree),
            pairs(&[("31", "a"), ("370", "c"), ("375", "d")])
        );

        let inverse = tree.apply(Effect::Remove { id: id("31") }).unwrap();
        assert!(matches!(inverse, Effect::Insert { displaced: Some(d), .. } if d == u4(7)));
        assert_eq!(layout(&tree), pairs(&[("30", "c"), ("35", "d")]));
        assert_compact(&tree, true);
    }

    #[test]
    fn remove_at_root_never_moves_anything() {
        let mut tree = TaskTree::new();
        insert(&mut tree, "1", "a", None);
        insert(&mut tree, "2", "b", None);
        let inverse = tree.apply(Effect::Remove { id: id("1") }).unwrap();
        assert!(matches!(
            inverse,
            Effect::Insert {
                displaced: None,
                ..
            }
        ));
        assert_eq!(layout(&tree), pairs(&[("2", "b")]));
    }

    #[test]
    fn invalid_removes_are_rejected_without_changes() {
        let mut tree = sample_tree();
        let before = snapshot(&tree);

        for at in ["5", "3", "37", "91", "310", "3f"] {
            assert!(
                matches!(
                    tree.apply(Effect::Remove { id: id(at) }),
                    Err(EffectError::Retrieval(RetrievalError::NoSuchId(_)))
                ),
                "remove at {at:?} was accepted"
            );
        }

        assert_eq!(snapshot(&tree), before);
    }

    #[test]
    fn get_task_requires_exact_leaf_path() {
        let tree = sample_tree();
        assert_eq!(tree.get_task(id("375").as_ref()).unwrap().name, "d");
        assert!(tree.get_task(id("37").as_ref()).is_err());
        assert!(tree.get_task(id("3755").as_ref()).is_err());
        assert!(tree.get_task(&[]).is_err());
    }

    #[test]
    fn optimise_collapses_legacy_chains() {
        // `3: tree { 5: tree { 1: a, 2: b } }`, as older versions could produce.
        let mut inner = TaskTree::new();
        inner.children[1] = Some(TaskTreeNode::Task(task("a")));
        inner.children[2] = Some(TaskTreeNode::Task(task("b")));
        let mut middle = TaskTree::new();
        middle.children[5] = Some(TaskTreeNode::Tree(Box::new(inner)));
        let mut tree = TaskTree::new();
        tree.children[3] = Some(TaskTreeNode::Tree(Box::new(middle)));
        tree.children[4] = Some(TaskTreeNode::Tree(Box::default()));

        tree.optimise();
        assert_eq!(layout(&tree), pairs(&[("31", "a"), ("32", "b")]));
        assert!(tree.children[4].is_none(), "empty subtrees are removed");
        assert_compact(&tree, true);

        let before = snapshot(&tree);
        tree.optimise();
        assert_eq!(snapshot(&tree), before, "optimise is idempotent");
    }

    // Effects.

    #[test]
    fn insert_and_remove_round_trip() {
        let mut tree = sample_tree();
        assert_round_trip(
            &mut tree,
            Effect::Insert {
                id: id("3f"),
                task: task("f"),
                displaced: Some(u4(6)),
            },
        );
        // The insert above moved the `3` subtree down to `36`.
        assert_round_trip(&mut tree, Effect::Remove { id: id("3670") });
        assert_round_trip(&mut tree, Effect::Remove { id: id("9") });
    }

    #[test]
    fn status_effects_round_trip() {
        let mut tree = sample_tree();
        assert_round_trip(
            &mut tree,
            Effect::Start {
                id: id("9"),
                at: at(9),
            },
        );
        assert_round_trip(
            &mut tree,
            Effect::Complete {
                id: id("9"),
                at: at(10),
            },
        );

        tree.apply(Effect::Start {
            id: id("31"),
            at: at(9),
        })
        .unwrap();
        assert_round_trip(&mut tree, Effect::Unstart { id: id("31") });
        assert_round_trip(
            &mut tree,
            Effect::Complete {
                id: id("31"),
                at: at(10),
            },
        );

        tree.apply(Effect::Complete {
            id: id("32"),
            at: at(10),
        })
        .unwrap();
        assert_round_trip(&mut tree, Effect::Uncomplete { id: id("32") });
    }

    #[test]
    fn edit_effect_round_trips() {
        let mut tree = sample_tree();
        assert_round_trip(
            &mut tree,
            Effect::Edit {
                id: id("375"),
                edit: TaskEdit {
                    name: Some("renamed".to_owned()),
                    due: Some(Some(at(12))),
                    repeat: Some(Some(Duration::days(1))),
                },
            },
        );
    }

    #[test]
    fn rejected_effects_leave_tree_unchanged() {
        let mut tree = sample_tree();
        tree.apply(Effect::Complete {
            id: id("9"),
            at: at(10),
        })
        .unwrap();
        let before = snapshot(&tree);

        let rejected = [
            Effect::Start {
                id: id("9"),
                at: at(9),
            },
            Effect::Complete {
                id: id("9"),
                at: at(9),
            },
            Effect::Unstart { id: id("9") },
            Effect::Uncomplete { id: id("31") },
            Effect::Start {
                id: id("5"),
                at: at(9),
            },
            Effect::Edit {
                id: id("31"),
                edit: TaskEdit {
                    repeat: Some(Some(Duration::days(1))),
                    ..TaskEdit::default()
                },
            },
        ];
        for effect in rejected {
            assert!(tree.apply(effect).is_err());
        }

        assert_eq!(snapshot(&tree), before);
    }

    // Commands.

    #[test]
    fn add_task_keeps_tree_compact() {
        let mut tree = TaskTree::new();
        let mut undo = Vec::new();
        for i in 0..500 {
            undo.push(tree.add_task(task(&format!("t{i}"))));
        }

        assert_compact(&tree, true);
        assert_eq!(tree.collect_tasks(&TaskId::new()).len(), 500);

        for record in undo.into_iter().rev() {
            tree.apply_record(record).unwrap();
            assert_compact(&tree, true);
        }
        assert_eq!(snapshot(&tree), snapshot(&TaskTree::new()));
    }

    #[test]
    fn add_task_displacing_a_task_is_undone_exactly() {
        let mut tree = TaskTree::new();
        for i in 0..16 {
            insert(&mut tree, &format!("{i:x}"), &format!("t{i}"), None);
        }
        let before = snapshot(&tree);

        // Every root slot is taken, so the new task must displace one of them.
        let record = tree.add_task(task("new"));
        assert!(
            tree.collect_tasks(&TaskId::new())
                .iter()
                .any(|(id, _)| id.as_ref().len() == 2)
        );
        assert_compact(&tree, true);

        assert_record_round_trip(&mut tree, &before, record);
    }

    #[test]
    fn delete_task_is_undone_exactly() {
        let mut tree = sample_tree();
        let before = snapshot(&tree);
        let record = tree.delete_task(id("370").as_ref()).unwrap();
        assert_record_round_trip(&mut tree, &before, record);

        assert!(tree.delete_task(id("5").as_ref()).is_err());
    }

    #[test]
    fn start_task_is_undone_exactly() {
        let mut tree = sample_tree();
        let before = snapshot(&tree);
        let record = tree.start_task(id("9").as_ref()).unwrap();
        assert!(matches!(
            tree.get_task(id("9").as_ref()).unwrap().status,
            TaskStatus::InProgress(_)
        ));
        assert_record_round_trip(&mut tree, &before, record);
    }

    #[test]
    fn edit_and_postpone_tasks_are_undone_exactly() {
        let mut tree = sample_tree();
        tree.edit_task(
            id("9").as_ref(),
            TaskEdit {
                due: Some(Some(at(9))),
                ..TaskEdit::default()
            },
        )
        .unwrap();

        let before = snapshot(&tree);
        let record = tree
            .postpone_task(id("9").as_ref(), Duration::hours(1))
            .unwrap();
        assert_record_round_trip(&mut tree, &before, record);

        let record = tree
            .edit_task(
                id("9").as_ref(),
                TaskEdit {
                    name: Some("renamed".to_owned()),
                    ..TaskEdit::default()
                },
            )
            .unwrap();
        assert_record_round_trip(&mut tree, &before, record);

        assert!(
            tree.postpone_task(id("31").as_ref(), Duration::hours(1))
                .is_err()
        );
    }

    #[test]
    fn complete_task_without_repeat_only_changes_status() {
        let mut tree = sample_tree();
        let before = snapshot(&tree);
        let record = tree.complete_task(id("9").as_ref()).unwrap();
        assert_eq!(record.0.len(), 1);
        assert_eq!(tree.collect_tasks(&TaskId::new()).len(), 5);
        assert_record_round_trip(&mut tree, &before, record);
    }

    #[test]
    fn complete_task_with_repeat_adds_next_occurrence() {
        let mut tree = TaskTree::new();
        tree.apply(Effect::Insert {
            id: id("3"),
            task: repeating("r"),
            displaced: None,
        })
        .unwrap();
        let before = snapshot(&tree);

        let record = tree.complete_task(id("3").as_ref()).unwrap();
        let tasks = tree.collect_tasks(&TaskId::new());
        assert_eq!(tasks.len(), 2);
        assert_eq!(
            tasks
                .iter()
                .filter(|(_, task)| task.status == TaskStatus::Incomplete)
                .count(),
            1
        );
        assert_record_round_trip(&mut tree, &before, record);
    }

    #[test]
    fn undoing_a_record_restores_ids_before_statuses() {
        // Completing `3` spawns a task that displaces it to `37`. The record's effects refer
        // to `3` as it was at each step, so undoing must remove the spawn first.
        let mut tree = TaskTree::new();
        tree.apply(Effect::Insert {
            id: id("3"),
            task: repeating("r"),
            displaced: None,
        })
        .unwrap();
        let before = snapshot(&tree);

        let complete = tree
            .apply(Effect::Complete {
                id: id("3"),
                at: at(10),
            })
            .unwrap();
        let spawn = tree
            .apply(Effect::Insert {
                id: id("35"),
                task: repeating("r"),
                displaced: Some(u4(7)),
            })
            .unwrap();
        assert!(matches!(
            tree.get_task(id("37").as_ref()).unwrap().status,
            TaskStatus::Complete(..)
        ));

        assert_record_round_trip(&mut tree, &before, Record(vec![complete, spawn]));
    }

    #[test]
    fn complete_task_with_repeat_survives_displacing_itself() {
        // With every root slot full, the spawned task displaces the completed one 1 in 16
        // times. Enough rounds make it near certain that case is covered.
        let mut displaced_itself = false;
        for _ in 0..300 {
            let mut tree = TaskTree::new();
            for i in 0..16 {
                let task = if i == 3 { repeating("r") } else { task("x") };
                tree.apply(Effect::Insert {
                    id: id(&format!("{i:x}")),
                    task,
                    displaced: None,
                })
                .unwrap();
            }
            let before = snapshot(&tree);

            let record = tree.complete_task(id("3").as_ref()).unwrap();
            displaced_itself |= tree.get_task(id("3").as_ref()).is_err();
            assert_compact(&tree, true);
            assert_record_round_trip(&mut tree, &before, record);
        }
        assert!(displaced_itself);
    }

    #[test]
    fn failed_command_leaves_tree_unchanged() {
        let mut tree = sample_tree();
        tree.complete_task(id("9").as_ref()).unwrap();
        let before = snapshot(&tree);

        assert!(tree.complete_task(id("9").as_ref()).is_err());
        assert!(tree.start_task(id("9").as_ref()).is_err());
        assert!(tree.complete_task(id("5").as_ref()).is_err());
        assert_eq!(snapshot(&tree), before);
    }

    #[test]
    fn records_survive_serialisation() {
        let mut tree = sample_tree();
        let before = snapshot(&tree);
        let record = tree.delete_task(id("370").as_ref()).unwrap();

        let json = serde_json::to_string(&record).unwrap();
        assert!(json.contains(r#""displaced":5"#), "{json}");
        let record: Record = serde_json::from_str(&json).unwrap();

        let redo = tree.apply_record(record).unwrap();
        assert_eq!(snapshot(&tree), before);

        let json = serde_json::to_string(&redo).unwrap();
        assert_eq!(json, r#"[{"Remove":{"id":"370"}}]"#);
    }

    #[test]
    fn random_command_history_undoes_and_redoes_exactly() {
        let mut tree = TaskTree::new();
        let mut snapshots = vec![snapshot(&tree)];
        let mut undo = Vec::new();

        for i in 0..400 {
            let ids: Vec<TaskId> = tree
                .collect_tasks(&TaskId::new())
                .into_iter()
                .map(|(id, _)| id)
                .collect();
            let target = (!ids.is_empty()).then(|| ids[random_range(0..ids.len())].clone());
            let name = format!("t{i}");

            let record = match (random_range(0..8), target) {
                (0..3, _) | (_, None) => Ok(tree.add_task(task(&name))),
                (3, _) => Ok(tree.add_task(repeating(&name))),
                (4, Some(id)) => tree.complete_task(id.as_ref()),
                (5, Some(id)) => tree.start_task(id.as_ref()),
                (6, Some(id)) => tree.delete_task(id.as_ref()),
                (_, Some(id)) => tree.edit_task(
                    id.as_ref(),
                    TaskEdit {
                        name: Some(name),
                        ..TaskEdit::default()
                    },
                ),
            };
            // Starting or completing a task twice is expected to fail.
            let Ok(record) = record else { continue };

            assert_compact(&tree, true);
            undo.push(record);
            snapshots.push(snapshot(&tree));
        }

        let mut redo = Vec::new();
        while let Some(record) = undo.pop() {
            redo.push(tree.apply_record(record).unwrap());
            assert_eq!(snapshot(&tree), snapshots[undo.len()]);
        }
        while let Some(record) = redo.pop() {
            undo.push(tree.apply_record(record).unwrap());
            assert_eq!(snapshot(&tree), snapshots[undo.len()]);
        }
    }

    // Display.

    #[test]
    fn list_is_sorted_filtered_and_aligned() {
        let mut tree = TaskTree::new();
        tree.apply(Effect::Insert {
            id: id("1"),
            task: Task::new("later".to_owned(), Some(at(12)), None).unwrap(),
            displaced: None,
        })
        .unwrap();
        tree.apply(Effect::Insert {
            id: id("2"),
            task: Task::new("sooner".to_owned(), Some(at(9)), None).unwrap(),
            displaced: None,
        })
        .unwrap();
        insert(&mut tree, "3", "whenever", None);
        insert(&mut tree, "4", "done", None);
        insert(&mut tree, "45", "busy", Some(0));
        tree.start_task(id("45").as_ref()).unwrap();
        tree.complete_task(id("40").as_ref()).unwrap();

        let filter = TaskFilter {
            status: Some(StatusFilter::Not(StatusKind::Complete)),
            due_before: None,
        };
        let output = tree.filtered(&filter).to_string();
        let names: Vec<&str> = output
            .lines()
            .map(|line| line.split_whitespace().find(|word| word.len() > 2).unwrap())
            .collect();
        assert_eq!(names, ["busy", "sooner", "later", "whenever"]);
        assert!(
            output
                .lines()
                .all(|line| line.starts_with(|c: char| c.is_ascii_hexdigit()))
        );
        assert!(output.lines().next().unwrap().starts_with("45   -  busy"));
        assert!(!output.contains("done"));
        assert!(
            tree.to_string().contains("done"),
            "unfiltered display shows everything"
        );
    }
}
