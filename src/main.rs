use {
    anyhow::Result,
    args::ManualFormat,
    crossterm::{
        cursor::MoveTo,
        terminal::{
            Clear,
            ClearType,
        },
    },
    parking_lot::RwLock,
    signal_hook::{
        consts::{
            SIGINT,
            SIGTERM,
        },
        iterator::Signals,
    },
    std::{
        collections::BTreeMap,
        io::{
            stdout,
            BufWriter,
            Write,
        },
        path::PathBuf,
        sync::Arc,
    },
    tokio::{
        process::Command,
        task::JoinSet,
    },
};

pub mod args;
pub mod reference;

#[tokio::main]
async fn main() -> Result<()> {
    let cmd = crate::args::ClapArgumentLoader::load()?;

    match cmd.command {
        | crate::args::Command::Manual { path, format } => {
            let out_path = PathBuf::from(path);
            std::fs::create_dir_all(&out_path)?;
            match format {
                | ManualFormat::Manpages => {
                    reference::build_manpages(&out_path)?;
                },
                | ManualFormat::Markdown => {
                    reference::build_markdown(&out_path)?;
                },
            }
            Ok(())
        },
        | crate::args::Command::Autocomplete { path, shell } => {
            let out_path = PathBuf::from(path);
            std::fs::create_dir_all(&out_path)?;
            reference::build_shell_completion(&out_path, &shell)?;
            Ok(())
        },
        | crate::args::Command::Multiplex { program, commands } => {
            let mut command_states = BTreeMap::<String, String>::new();
            for command in commands.iter() {
                command_states.insert(command.clone(), "PENDING".to_owned());
            }
            let command_states = Arc::new(RwLock::new(command_states));

            fn draw_state(state: &BTreeMap<String, String>, in_progress: bool) {
                let mut writer = BufWriter::new(stdout());
                crossterm::queue!(writer, Clear(ClearType::All)).unwrap();
                crossterm::queue!(writer, MoveTo(0, 0)).unwrap();

                writeln!(writer, "Executing commands:").unwrap();
                for item in state.iter() {
                    writeln!(writer, "⇒ {}", item.0).unwrap();
                    writeln!(writer, " ↳ Status: {}", item.1).unwrap();
                }

                writeln!(writer, "").unwrap(); // new line
                write!(writer, "Thinking...").unwrap();
                if !in_progress {
                    write!(writer, " DONE").unwrap();
                }
                writeln!(writer, "").unwrap();
                writer.flush().unwrap();
            }

            let (report_tx, report_rx) = flume::unbounded::<Option<(String, String)>>();

            // reporting task
            let report_command_states = command_states.clone();
            let report_fut = tokio::spawn(async move {
                for update in report_rx.iter() {
                    if let Some((cmd, state)) = update {
                        report_command_states.write().insert(cmd, state);
                    }
                    draw_state(&report_command_states.read(), true);
                }
            });
            report_tx.send(None).unwrap(); // first draw

            let mut joins = JoinSet::new();
            for command in commands {
                let report_channel = report_tx.clone();
                // first item is shell to execute commands in (like "/bin/sh")
                let mut cmd_proc = Command::new(&program[0]);
                // remaining items are arguments to shell (like "-c")
                for arg in &program[1..] {
                    cmd_proc.arg(arg);
                }
                // final argument is the command itself
                cmd_proc.arg(&command);

                // No STDIN, STDOUT or STDERR are used
                cmd_proc.stdin(std::process::Stdio::null());
                cmd_proc.stdout(std::process::Stdio::null());
                cmd_proc.stderr(std::process::Stdio::null());

                // spawn child process as member of JoinSet
                joins.spawn(async move {
                    let mut child_proc = cmd_proc.spawn().unwrap();
                    // ignore error
                    let _ = report_channel.send(Some((command.clone(), "RUNNING".to_owned())));
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
                    let _ = report_channel.send(Some((command.clone(), status)));
                });
            }
            drop(report_tx);

            let mut signals = Signals::new([SIGINT, SIGTERM]).unwrap();
            let signals_handle = signals.handle();

            // task handling abort signals
            let abort_fut = tokio::spawn(async move { signals.wait() });
            // task handling command execution
            let command_fut = tokio::spawn(async move { while let Some(_) = joins.join_next().await {} });
            tokio::select! {
                _ = abort_fut => {}, // abort signal was received
                _ = command_fut => {}, // all tasks were executed
                _ = report_fut => {}, // reporting task failed
            }
            draw_state(&command_states.read(), false);
            signals_handle.close();

            Ok(())
        },
    }
}
