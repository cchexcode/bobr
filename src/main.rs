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
    signal_hook::{
        consts::{
            SIGINT,
            SIGTERM,
        },
        iterator::Signals,
    },
    std::{
        collections::HashMap,
        io::{
            stdout,
            BufWriter,
            Write,
        },
        path::PathBuf,
        thread::sleep,
        time::Duration,
    },
    tokio::{
        process::Command,
        task::{
            yield_now,
            JoinSet,
        },
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
        | crate::args::Command::Multiplex { commands } => {
            let mut command_states = HashMap::<String, String>::new();
            for command in commands.iter() {
                command_states.insert(command.clone(), "PENDING".to_owned());
            }

            let (report_tx, report_rx) = flume::unbounded::<Option<(String, String)>>();
            let report_fut = tokio::spawn(async move {
                for update in report_rx.iter() {
                    yield_now().await; // make sure it's abortable
                    if let Some((cmd, state)) = update {
                        command_states.insert(cmd, state);
                    }

                    let mut writer = BufWriter::new(stdout());
                    crossterm::queue!(writer, Clear(ClearType::All)).unwrap();
                    crossterm::queue!(writer, MoveTo(0, 0)).unwrap();

                    writeln!(writer, "Executing commands:").unwrap();
                    for item in command_states.iter() {
                        writeln!(writer, "⇒ {}", item.0).unwrap();
                        writeln!(writer, " ↳ Status: {}", item.1).unwrap();
                    }
                    writer.flush().unwrap();
                    sleep(Duration::from_secs(1));
                }
            });
            report_tx.send(None).unwrap(); // first draw

            let mut joins = JoinSet::new();
            for command in commands {
                let report_channel = report_tx.clone();
                joins.spawn(async move {
                    let mut cmd_proc = Command::new("sh");
                    cmd_proc.args(&["-c", &command]);
                    cmd_proc.stdin(std::process::Stdio::null());
                    cmd_proc.stdout(std::process::Stdio::null());
                    cmd_proc.stderr(std::process::Stdio::null());
                    let mut child_proc = cmd_proc.spawn().unwrap();
                    let exit_code = child_proc.wait().await.unwrap();
                    let status = if exit_code.success() {
                        "SUCCESS".to_owned()
                    } else {
                        format!("FAILED ({})", exit_code.code().unwrap())
                    };
                    report_channel.send(Some((command.clone(), status))).unwrap();
                });
            }
            drop(report_tx);

            let mut signals = Signals::new([SIGINT, SIGTERM]).unwrap();
            let signals_handle = signals.handle();
            let abort_fut = tokio::spawn(async move { signals.wait() });
            let command_fut = tokio::spawn(async move { while let Some(_) = joins.join_next().await {} });
            tokio::select! {
                _ = abort_fut => {
                    println!("signal received... aborting...");
                },
                _ = command_fut => {
                    println!("completed all tasks... shutting down...")
                },
                _ = report_fut => {
                },
            }
            signals_handle.close();

            Ok(())
        },
    }
}
