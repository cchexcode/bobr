use std::path::PathBuf;

use anyhow::Result;
use args::ManualFormat;
use multiplexer::Multiplexer;

pub mod args;
pub mod multiplexer;
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
        | crate::args::Command::Multiplex {
            program,
            stderr,
            stdout,
            commands,
        } => {
            Multiplexer::new(program, stderr, stdout, commands).run().await?;
            Ok(())
        },
    }
}

#[cfg(test)]
mod test {
    use anyhow::Result;
    use chrono::Duration;
    use clitest::CliTestSetup;

    use crate::multiplexer::StdoutData;

    fn setup_test() -> CliTestSetup {
        CliTestSetup::new()
    }

    #[tokio::test]
    pub async fn test_smoke() -> Result<()> {
        let setup = setup_test();
        // run CLI with experimental flag, test file and stdout output (json formatted)
        let result = setup.run("-e -f ./test/example.sh --stdout=json")?;

        // assert program ran and completed successfully
        assert!(result.status.success());

        // smoke test parse stdout data
        let result_typed = serde_json::from_slice::<StdoutData>(&result.stdout)?;

        // assert commands are run in parallel and reasonably fast
        let runtime = result_typed.metadata.ended - result_typed.metadata.started;
        assert!(runtime > Duration::milliseconds(1000));
        assert!(runtime <= Duration::milliseconds(1100));

        // assert stdout output of subcommands
        assert_eq!(3, result_typed.tasks.len());
        assert_eq!("", result_typed.tasks.get(&0).unwrap().stdout);
        assert_eq!("", result_typed.tasks.get(&1).unwrap().stdout);
        assert_eq!("test\n", result_typed.tasks.get(&2).unwrap().stdout);

        Ok(())
    }
}
