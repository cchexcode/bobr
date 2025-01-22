use std::{
    collections::{BTreeMap, VecDeque},
    io::{stderr, BufWriter, Write},
    sync::Arc,
};

use anyhow::Result;
use chrono::{DateTime, Utc};
use crossterm::{
    cursor::MoveTo,
    style::{Print, Stylize},
    terminal::{Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};
use flume::Receiver;
use parking_lot::RwLock;
use signal_hook::{
    consts::{SIGINT, SIGTERM},
    iterator::Signals,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    process::Command,
    sync::Semaphore,
    task::JoinSet,
};

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct MultiplexerResult {
    pub metadata: MultiplexerResultMetadata,
    pub tasks: BTreeMap<usize, MultiplexerResultDataTask>,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct MultiplexerResultMetadata {
    pub started: DateTime<Utc>,
    pub ended: DateTime<Utc>,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct MultiplexerResultDataTask {
    pub stdout: String,
}

#[derive(Debug, Eq, PartialEq)]
enum TaskStatusCompleted {
    Success,
    Failed(Option<i32>),
}

#[derive(Debug, Eq, PartialEq)]
enum TaskStatus {
    Pending,
    Running,
    Completed(TaskStatusCompleted),
}

enum TaskEvent {
    Update { id: usize, status: TaskStatus },
    Stderr { id: usize, line: String },
    Stdout { id: usize, content: String },
}

struct Task {
    command: String,
    status: TaskStatus,
    stderr: VecDeque<String>,
    stdout: String,
}

pub struct Multiplexer {
    program: Vec<String>,
    stderr: usize,
    tasks: BTreeMap<usize, RwLock<Task>>,
    parallelism: usize,
}

impl Multiplexer {
    pub fn new(program: Vec<String>, stderr: usize, tasks: Vec<String>, processes: usize) -> Self {
        let mut task_map = BTreeMap::<usize, RwLock<Task>>::new();
        for i in 0..tasks.len() {
            task_map.insert(
                i,
                RwLock::new(Task {
                    command: tasks[i].clone(),
                    status: TaskStatus::Pending,
                    stderr: VecDeque::<_>::new(),
                    stdout: String::new(),
                }),
            );
        }

        Self {
            program,
            stderr,
            tasks: task_map,
            parallelism: processes,
        }
    }

    pub async fn run(self) -> Result<MultiplexerResult> {
        let time_start = Utc::now();
        let (task_event_tx, task_event_rx) = flume::unbounded::<TaskEvent>();

        let mut joins = JoinSet::new();
        let budget = Arc::new(Semaphore::new(self.parallelism));
        for command in self.tasks.iter() {
            let report_channel = task_event_tx.clone();
            // first item is shell to execute commands in (like "/bin/sh")
            let mut cmd_proc = Command::new(&self.program[0]);
            // remaining items are arguments to shell (like "-c")
            for arg in &self.program[1..] {
                cmd_proc.arg(arg);
            }
            // final argument is the command itself
            cmd_proc.arg(&command.1.read().command);

            cmd_proc.stdin(std::process::Stdio::null());
            cmd_proc.stdout(std::process::Stdio::piped());
            cmd_proc.stderr(std::process::Stdio::piped());

            // spawn child process as member of JoinSet
            let task_id = command.0.clone();
            let task_budget = budget.clone();
            joins.spawn(async move {
                let _seq_lock = task_budget.acquire().await;
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

                let stdout = child_proc.stdout.take().unwrap();
                let mut stdout_out = String::new();
                let mut stdout_reader = BufReader::new(stdout);
                stdout_reader.read_to_string(&mut stdout_out).await.unwrap();
                let _ = report_channel.send(TaskEvent::Stdout {
                    id: task_id.clone(),
                    content: stdout_out,
                });

                let exit_code = child_proc.wait().await.unwrap();
                let status = if exit_code.success() {
                    TaskStatusCompleted::Success
                } else {
                    TaskStatusCompleted::Failed(exit_code.code())
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

        let event_handler = TaskEventReporter {
            rx: task_event_rx,
            stderr: self.stderr,
            tasks: &self.tasks,
        };

        tokio::select! {
            _ = abort_fut => {
                return Err(anyhow::anyhow!("user interrupt"));
            }, // abort signal was received
            _ = command_fut => {}, // all tasks were executed
            _ = event_handler.run() => {}, // reporting task failed
        }
        signals_handle.close();
        let time_end = Utc::now();

        let mut data = MultiplexerResult {
            metadata: MultiplexerResultMetadata {
                started: time_start,
                ended: time_end,
            },
            tasks: BTreeMap::<_, _>::new(),
        };
        for t in self.tasks.into_iter() {
            let task = t.1.into_inner();
            data.tasks
                .insert(t.0.clone(), MultiplexerResultDataTask { stdout: task.stdout });
        }

        Ok(data)
    }
}

struct TaskEventReporter<'a> {
    rx: Receiver<TaskEvent>,
    stderr: usize,
    tasks: &'a BTreeMap<usize, RwLock<Task>>,
}

impl<'a> TaskEventReporter<'a> {
    pub async fn run(self) {
        let mut remaining = self.tasks.len();
        crossterm::execute!(std::io::stderr(), EnterAlternateScreen).unwrap();
        for event in self.rx {
            match event {
                | TaskEvent::Update { id, status } => {
                    match &status {
                        | TaskStatus::Completed(_) => remaining -= 1,
                        | _ => {},
                    }
                    self.tasks.get(&id).unwrap().write().status = status;
                },
                | TaskEvent::Stderr { id, line } => {
                    let stderr = &mut self.tasks.get(&id).unwrap().write().stderr;
                    stderr.push_back(line);
                    if stderr.len() > self.stderr {
                        stderr.pop_front();
                    }
                },
                | TaskEvent::Stdout { id, content } => {
                    let task = &mut self.tasks.get(&id).unwrap().write();
                    task.stdout = content;
                },
            }

            // last should be printed to stderr, therefore exit alternate screen before last
            // draw
            if remaining == 0 {
                crossterm::execute!(std::io::stderr(), LeaveAlternateScreen).unwrap();
            }
            Self::draw(&self.tasks, remaining == 0);
        }
    }

    fn draw(tasks: &BTreeMap<usize, RwLock<Task>>, completed: bool) {
        let mut writer = BufWriter::new(stderr());
        if !completed {
            crossterm::queue!(writer, Clear(ClearType::All)).unwrap();
            crossterm::queue!(writer, MoveTo(0, 0)).unwrap();
        }

        for item in tasks.iter() {
            let task = item.1.read();
            crossterm::queue!(writer, Print(format!("⇒ ({}) {}\n", item.0, task.command.trim()))).unwrap();
            let status = match &task.status {
                | TaskStatus::Pending => "PENDING".to_owned().yellow(),
                | TaskStatus::Running => "RUNNING".to_owned().yellow(),
                | TaskStatus::Completed(v) => {
                    match v {
                        | TaskStatusCompleted::Success => "SUCCESS (0)".to_owned().green(),
                        | TaskStatusCompleted::Failed(code) => {
                            format!(
                                "FAILED ({})",
                                code.map(|v| v.to_string()).unwrap_or("unknown".to_owned())
                            )
                            .red()
                        },
                    }
                },
            };
            crossterm::queue!(writer, Print(" ↳ Status: ")).unwrap();
            crossterm::queue!(writer, Print(status)).unwrap();
            crossterm::queue!(writer, Print("\n")).unwrap();

            if task.stderr.len() > 0 {
                crossterm::queue!(writer, Print(" ↳ Stderr: \n")).unwrap();
                for line in &task.stderr {
                    crossterm::queue!(writer, Print(format!("   |> {}\n", line))).unwrap();
                }
            }
        }

        crossterm::queue!(writer, Print("\n")).unwrap();
        crossterm::queue!(writer, Print("Thinking...")).unwrap();
        if completed {
            crossterm::queue!(writer, Print(" DONE\n")).unwrap();
        }
        writer.flush().unwrap();
    }
}
