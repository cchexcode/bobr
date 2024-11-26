use std::{
    collections::{BTreeMap, VecDeque},
    io::{stdout, BufWriter, Write},
    sync::Arc,
};

use anyhow::Result;
use crossterm::{
    cursor::MoveTo,
    terminal::{Clear, ClearType},
};
use flume::Receiver;
use parking_lot::RwLock;
use signal_hook::{
    consts::{SIGINT, SIGTERM},
    iterator::Signals,
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    task::JoinSet,
};

#[derive(Debug, Eq, PartialEq)]
enum TaskStatus {
    Pending,
    Running,
    Completed(String),
}
enum TaskEvent {
    Update { id: usize, status: TaskStatus },
    Stderr { id: usize, line: String },
}

pub struct Task {
    command: String,
    status: TaskStatus,
    stderr: VecDeque<String>,
}

pub struct Multiplexer {
    program: Vec<String>,
    stderr: usize,
    tasks: Arc<RwLock<BTreeMap<usize, Task>>>,
}

impl Multiplexer {
    pub fn new(program: Vec<String>, stderr: usize, tasks: Vec<String>) -> Self {
        let task_map = Arc::new(RwLock::new(BTreeMap::<usize, Task>::new()));
        for i in 0..tasks.len() {
            task_map.write().insert(i, Task {
                command: tasks[i].clone(),
                status: TaskStatus::Pending,
                stderr: VecDeque::<_>::new(),
            });
        }

        Self {
            program,
            stderr,
            tasks: task_map,
        }
    }

    pub async fn run(self) -> Result<()> {
        let (task_event_tx, task_event_rx) = flume::unbounded::<TaskEvent>();

        let event_handler = TaskEventHandler {
            rx: task_event_rx,
            stderr: self.stderr,
            tasks: self.tasks.clone(),
        };
        let event_handler_fut = tokio::spawn(async move {
            event_handler.run().await;
        });

        let mut joins = JoinSet::new();
        for command in self.tasks.read().iter() {
            let report_channel = task_event_tx.clone();
            // first item is shell to execute commands in (like "/bin/sh")
            let mut cmd_proc = Command::new(&self.program[0]);
            // remaining items are arguments to shell (like "-c")
            for arg in &self.program[1..] {
                cmd_proc.arg(arg);
            }
            // final argument is the command itself
            cmd_proc.arg(&command.1.command);

            cmd_proc.stdin(std::process::Stdio::null());
            cmd_proc.stdout(std::process::Stdio::piped());
            cmd_proc.stderr(std::process::Stdio::piped());

            // spawn child process as member of JoinSet
            let task_id = command.0.clone();
            joins.spawn(async move {
                let mut child_proc = cmd_proc.spawn().unwrap();
                // ignore error
                let _ = report_channel.send(TaskEvent::Update {
                    id: task_id.clone(),
                    status: TaskStatus::Running,
                });

                let stderr = child_proc.stderr.take().unwrap();
                let mut stderr_reader = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = stderr_reader.next_line().await {
                    let _ = report_channel.send(TaskEvent::Stderr {
                        id: task_id.clone(),
                        line,
                    });
                }

                let exit_code = child_proc.wait().await.unwrap();
                let status = if exit_code.success() {
                    "SUCCESS (0)".to_owned()
                } else {
                    format!(
                        "FAILED ({})",
                        exit_code.code().map(|v| v.to_string()).unwrap_or("unknown".to_owned())
                    )
                };
                // ignore error
                let _ = report_channel.send(TaskEvent::Update {
                    id: task_id.clone(),
                    status: TaskStatus::Completed(status),
                });
            });
        }
        drop(task_event_tx);

        let mut signals = Signals::new([SIGINT, SIGTERM]).unwrap();
        let signals_handle = signals.handle();

        // task handling abort signals
        let abort_fut = tokio::spawn(async move { signals.wait() });
        // task handling command execution
        let command_fut = tokio::spawn(async move { while let Some(_) = joins.join_next().await {} });
        tokio::select! {
            _ = abort_fut => {}, // abort signal was received
            _ = command_fut => {}, // all tasks were executed
            _ = event_handler_fut => {}, // reporting task failed
        }
        signals_handle.close();

        Ok(())
    }
}

struct TaskEventHandler {
    rx: Receiver<TaskEvent>,
    stderr: usize,
    tasks: Arc<RwLock<BTreeMap<usize, Task>>>,
}

impl TaskEventHandler {
    pub async fn run(self) {
        let mut remaining = self.tasks.read().len();
        for event in self.rx {
            match event {
                | TaskEvent::Update { id, status } => {
                    match &status {
                        | TaskStatus::Completed(_) => remaining -= 1,
                        | _ => {},
                    }
                    self.tasks.write().get_mut(&id).unwrap().status = status;
                },
                | TaskEvent::Stderr { id, line } => {
                    let mut lock = self.tasks.write();
                    let stderr = &mut lock.get_mut(&id).unwrap().stderr;
                    stderr.push_back(line);
                    if stderr.len() > self.stderr {
                        stderr.pop_front();
                    }
                },
            }
            Self::draw(&self.tasks.read(), remaining == 0);
        }
    }

    fn draw(tasks: &BTreeMap<usize, Task>, completed: bool) {
        let mut writer = BufWriter::new(stdout());
        crossterm::queue!(writer, Clear(ClearType::All)).unwrap();
        crossterm::queue!(writer, Clear(ClearType::Purge)).unwrap();
        crossterm::queue!(writer, MoveTo(0, 0)).unwrap();

        writeln!(writer, "Executing commands:").unwrap();
        for item in tasks.iter() {
            writeln!(writer, "⇒ ({}) {}", item.0, item.1.command).unwrap();
            let status = match &item.1.status {
                | TaskStatus::Pending => "PENDING",
                | TaskStatus::Running => "RUNNING",
                | TaskStatus::Completed(v) => v,
            };
            writeln!(writer, " ↳ Status: {}", status).unwrap();

            if item.1.stderr.len() > 0 {
                writeln!(writer, " ↳ Stderr:").unwrap();
                for line in &item.1.stderr {
                    writeln!(writer, "   |> {}", line).unwrap();
                }
            }
            writeln!(writer, "").unwrap();
        }

        writeln!(writer, "").unwrap(); // new line
        write!(writer, "Thinking...").unwrap();
        if completed {
            write!(writer, " DONE").unwrap();
        }
        writeln!(writer, "").unwrap();
        writer.flush().unwrap();
    }
}
