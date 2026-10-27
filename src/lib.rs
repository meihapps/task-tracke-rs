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

#[derive(Serialize, Deserialize, Ord, PartialOrd, Eq, PartialEq)]
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
    #[error("Invalid duration '{0}' ({1}). Try e.g. '30m', '2h' or '1w 2d'.")]
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
        let mut inverse = record
            .0
            .into_iter()
            .rev()
            .map(|effect| self.apply(effect))
            .collect::<Result<Vec<_>, _>>()?;
        // The inverses were produced last step first, but a record lists steps in the order they happened.
        inverse.reverse();
        Ok(Record(inverse))
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
            self.repeat
                .and_then(|x| Some(humantime::Duration::from(x.to_std().ok()?).to_string()))
                .unwrap_or("n/a".to_owned())
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
