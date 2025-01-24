use std::path::PathBuf;

use anyhow::Result;
use args::{ManualFormat, StdoutFormat};
use multiplexer::Multiplexer;

pub mod args;
pub mod config;
pub mod multiplexer;
pub mod reference;

#[deny(unsafe_code)]
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
        | crate::args::Command::Multiplex {
            program,
            stderr,
            stdout,
            commands,
            parallelism,
        } => {
            let parallelism = parallelism.unwrap_or(commands.len());
            let result = Multiplexer::new(program, stderr, commands, parallelism).run().await?;
            if let Some(v) = stdout {
                match v {
                    #[cfg(feature = "format+json")]
                    | StdoutFormat::Json => {
                        serde_json::to_writer(std::io::stdout(), &result)?;
                    },
                    #[cfg(feature = "format+yaml")]
                    | StdoutFormat::Yaml => {
                        serde_yml::to_writer(std::io::stdout(), &result)?;
                    },
                }
            }
            Ok(())
        },
    }
}

#[cfg(test)]
mod test {
    use anyhow::Result;
    use chrono::Duration;
    use clitest::CliTestSetup;

    use crate::multiplexer::MultiplexerResult;

    fn setup_test() -> CliTestSetup {
        let mut setup = CliTestSetup::new();
        setup.with_env("RUST_BACKTRACE", "0");
        setup
    }

    #[tokio::test]
    pub async fn test_smoke() -> Result<()> {
        let mut setup = setup_test();
        setup.with_cargo_flag("--features=\"format+toml\"");

        // run CLI with experimental flag, test file and stdout output (json formatted)
        let result = setup.run("-e -f ./test/example.toml --stdout=json")?;

        // assert program ran and completed successfully
        assert!(result.status.success());

        // smoke test parse stdout data
        let result_typed = serde_json::from_slice::<MultiplexerResult>(&result.stdout)?;

        // assert commands are run in parallel and reasonably fast
        let runtime = result_typed.metadata.ended - result_typed.metadata.started;
        assert!(runtime > Duration::milliseconds(1000));
        assert!(runtime <= Duration::milliseconds(1200));

        // assert stdout output of subcommands
        assert_eq!(3, result_typed.tasks.len());
        assert_eq!("", result_typed.tasks.get(&0).unwrap().stdout);
        assert_eq!("", result_typed.tasks.get(&1).unwrap().stdout);
        assert_eq!("test\n", result_typed.tasks.get(&2).unwrap().stdout);

        Ok(())
    }

    #[tokio::test]
    pub async fn test_cmd_exec_experimental_stdout() -> Result<()> {
        let setup = setup_test();
        // without experimental flag
        let result = setup.run("--stdout=json")?;
        assert!(!result.status.success()); // can not succeed

        let stderr = result.stderr_str();
        let stderr_last = stderr.lines().last().unwrap();
        assert_eq!("Error: experimental flag (stdout)", stderr_last);

        // with experimental flag
        let result = setup.run("-e --stdout=json")?;
        assert!(result.status.success()); // must succeed

        Ok(())
    }

    #[tokio::test]
    pub async fn test_cmd_exec_experimental_parallelism() -> Result<()> {
        let setup = setup_test();
        // without experimental flag
        let result = setup.run("-p4")?;
        assert!(!result.status.success()); // can not succeed

        let stderr = result.stderr_str();
        let stderr_last = stderr.lines().last().unwrap();
        assert_eq!("Error: experimental flag (parallelism)", stderr_last);

        // with experimental flag
        let result = setup.run("-e -p4")?;
        assert!(result.status.success()); // must succeed

        Ok(())
    }

    #[tokio::test]
    pub async fn test_feature_format_yaml() -> Result<()> {
        // run without feature
        let result = setup_test().run("-e --stdout=yaml")?;
        assert!(!result.status.success()); // can not succeed

        // run with format+yaml feature
        let result = setup_test()
            .with_cargo_flag("--features=\"format+yaml\"")
            .run("-e --stdout=yaml")?;
        assert!(result.status.success()); // must succeed

        Ok(())
    }
}
