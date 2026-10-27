use chrono::{DateTime, Duration, Local};
use rand::random_range;
use serde::{Deserialize, Serialize};
use std::{
    cmp::max,
    fmt::{self, Write},
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

pub struct TaskId(Vec<U4>);

#[derive(Serialize, Deserialize, Ord, PartialOrd, Eq, PartialEq)]
pub enum TaskStatus {
    InProgress(DateTime<Local>),
    Incomplete,
    Complete(Option<DateTime<Local>>, DateTime<Local>),
}

#[derive(Debug, Error)]
pub enum StatusError {
    #[error("Task already started.")]
    TaskAlreadyStarted,
    #[error("Task already completed.")]
    TaskAlreadyCompleted,
    #[error("Attempted to postpone a task with no due date.")]
    PostponedTaskWithNoDueDate,
}

#[derive(Debug, Error)]
pub enum RetrievalError {
    #[error("No such id.")]
    NoSuchId,
    #[error("Invalid hex string.")]
    InvalidHexString,
}

impl TaskTree {
    #[must_use]
    pub fn new() -> TaskTree {
        TaskTree {
            children: std::array::from_fn(|_| Option::default()),
        }
    }

    pub fn add_task(&mut self, task: Task) {
        let position = random_range(0..16);

        match self.children[position].take() {
            None => {
                self.children[position] = Some(TaskTreeNode::Task(task));
            }
            Some(TaskTreeNode::Tree(mut tree)) => {
                tree.add_task(task);
                self.children[position] = Some(TaskTreeNode::Tree(tree));
            }
            Some(TaskTreeNode::Task(other_task)) => {
                let mut child_tree = TaskTree::new();
                child_tree.add_task(other_task);
                child_tree.add_task(task);
                self.children[position] = Some(TaskTreeNode::Tree(Box::new(child_tree)));
            }
        }
    }

    /// Deletes the task associated with the provided id, then optimises the tree. Returns the deleted task.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The provided ID does not exist.
    pub fn delete_task(&mut self, id: &[U4]) -> Result<Task, RetrievalError> {
        let task = self.delete_task_inner(id)?;
        self.optimise();
        Ok(task)
    }

    /// Deletes the task associated with the provided id.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The provided ID does not exist.
    pub fn delete_task_inner(&mut self, id: &[U4]) -> Result<Task, RetrievalError> {
        let Some((&index, rest)) = id.split_first() else {
            return Err(RetrievalError::NoSuchId);
        };

        let index = usize::from(index);

        let Some(node) = self.children.get(index).and_then(Option::as_ref) else {
            return Err(RetrievalError::NoSuchId);
        };

        match node {
            TaskTreeNode::Task(_) => {
                if !rest.is_empty() {
                    return Err(RetrievalError::NoSuchId);
                }

                match self.children[index].take() {
                    Some(TaskTreeNode::Task(task)) => Ok(task),
                    Some(node) => {
                        self.children[index] = Some(node);
                        Err(RetrievalError::NoSuchId)
                    }
                    None => Err(RetrievalError::NoSuchId),
                }
            }

            TaskTreeNode::Tree(_) => {
                let Some(TaskTreeNode::Tree(tree)) = self.children[index].as_mut() else {
                    return Err(RetrievalError::NoSuchId);
                };

                tree.delete_task_inner(rest)
            }
        }
    }

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
        match (
            &self.children[*id.first().ok_or(RetrievalError::NoSuchId)? as usize],
            id.len(),
        ) {
            (Some(TaskTreeNode::Task(task)), 1) => Ok(task),
            (Some(TaskTreeNode::Tree(_)), 1) | (Some(TaskTreeNode::Task(_)) | None, _) => {
                Err(RetrievalError::NoSuchId)
            }
            (Some(TaskTreeNode::Tree(tree)), _) => tree.get_task(&id[1..]),
        }
    }

    /// Gets a task from the task tree by id.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - There is no task with the provided ID.
    pub fn get_task_mut(&mut self, id: &[U4]) -> Result<&mut Task, RetrievalError> {
        match (
            &mut self.children[*id.first().ok_or(RetrievalError::NoSuchId)? as usize],
            id.len(),
        ) {
            (Some(TaskTreeNode::Task(task)), 1) => Ok(task),
            (Some(TaskTreeNode::Tree(_)), 1) | (Some(TaskTreeNode::Task(_)) | None, _) => {
                Err(RetrievalError::NoSuchId)
            }
            (Some(TaskTreeNode::Tree(tree)), _) => tree.get_task_mut(&id[1..]),
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
    ) -> fmt::Result {
        let mut tasks = self.collect_tasks(prefix);

        tasks.sort_by(|a, b| {
            a.1.status
                .cmp(&b.1.status)
                .then_with(|| b.1.due.cmp(&a.1.due))
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
    pub fn collect_tasks(&self, prefix: &TaskId) -> Vec<(TaskId, &Task)> {
        let mut tasks = Vec::new();

        for (i, child) in self.children.iter().enumerate() {
            let Some(child) = child else {
                continue;
            };

            debug_assert!(i < 16);
            let index =
                u4::U4::try_from(i).unwrap_or_else(|_| unreachable!("Array is of length 16."));

            let mut id = prefix.clone();
            id.push(index);

            match child {
                TaskTreeNode::Task(task) => tasks.push((id, task)),
                TaskTreeNode::Tree(tree) => tasks.extend(tree.collect_tasks(&id)),
            }
        }

        tasks
    }

    /// Purges all completed tasks.
    ///
    /// # Errors
    ///
    /// This function will return an error if .
    pub fn purge(&mut self) -> Result<Vec<Task>, RetrievalError> {
        let ids: Vec<TaskId> = self
            .collect_tasks(&TaskId::new())
            .into_iter()
            .filter(|(_, task)| matches!(task.status, TaskStatus::Complete(_, _)))
            .map(|(id, _)| id)
            .collect();

        let tasks = ids
            .into_iter()
            .map(|id| self.delete_task_inner(id.as_ref()))
            .collect();

        self.optimise();

        tasks
    }
}

impl Default for TaskTree {
    fn default() -> Self {
        Self::new()
    }
}

impl Task {
    #[must_use]
    pub fn new(
        name: String,
        due_date: Option<DateTime<Local>>,
        recurrence: Option<Duration>,
    ) -> Task {
        Task {
            creation_date: Local::now(),
            due: due_date,
            name,
            status: TaskStatus::Incomplete,
            repeat: recurrence,
        }
    }

    /// Sets `status` to `InProgress`.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The task is already in progress.
    /// - The task is already completed.
    pub fn start_task(&mut self) -> Result<(), StatusError> {
        match self.status {
            TaskStatus::Incomplete => {
                self.status = TaskStatus::InProgress(Local::now());
                Ok(())
            }
            TaskStatus::InProgress(_) => Err(StatusError::TaskAlreadyStarted),
            TaskStatus::Complete(_, _) => Err(StatusError::TaskAlreadyCompleted),
        }
    }

    /// Sets status to `Complete`.
    /// Will generate a new task if the task is recurring.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The task is already completed.
    pub fn complete_task(&mut self) -> Result<Option<Task>, StatusError> {
        let next = if let (Some(recurrence), Some(due_date)) = (self.repeat, self.due) {
            Some(Task::new(
                self.name.clone(),
                Some(due_date + recurrence),
                self.repeat,
            ))
        } else {
            None
        };

        match self.status {
            TaskStatus::Incomplete => {
                self.status = TaskStatus::Complete(None, Local::now());
                Ok(next)
            }
            TaskStatus::InProgress(start_time) => {
                self.status = TaskStatus::Complete(Some(start_time), Local::now());
                Ok(next)
            }
            TaskStatus::Complete(_, _) => Err(StatusError::TaskAlreadyCompleted),
        }
    }

    /// Postpones a task by the given duration.
    /// If the due date is in the past, it will postpone relative to current time.
    /// If the due date is in the future, it will postpone relative to due date.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The task has no due date.
    pub fn postpone_task(&mut self, duration: chrono::TimeDelta) -> Result<(), StatusError> {
        if let Some(due) = self.due {
            self.due = Some(max(due, Local::now()) + duration);
            Ok(())
        } else {
            Err(StatusError::PostponedTaskWithNoDueDate)
        }
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

impl From<std::num::TryFromIntError> for RetrievalError {
    fn from(_: std::num::TryFromIntError) -> Self {
        RetrievalError::InvalidHexString
    }
}

impl fmt::Display for TaskTree {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let id_width = self.max_id_len().next_multiple_of(4);
        self.fmt_helper(f, &TaskId::new(), id_width)
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
