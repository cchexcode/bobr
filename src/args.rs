use std::{collections::HashMap, io::Read, str::FromStr};

use anyhow::{anyhow, Result};
use clap::ArgAction;
use itertools::Itertools;

use crate::config::Config;

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Privilege {
    Normal,
    Experimental,
}

#[derive(Debug)]
pub(crate) struct CallArgs {
    pub privileges: Privilege,
    pub command: Command,
}

impl CallArgs {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.privileges == Privilege::Experimental {
            return Ok(());
        }

        match &self.command {
            | Command::Multiplex {
                stdout, parallelism, ..
            } => {
                match stdout {
                    | Some(..) => Err(anyhow!("experimental flag (stdout)")),
                    | None => Ok(()),
                }?;

                match parallelism {
                    | Some(..) => Err(anyhow!("experimental flag (parallelism)")),
                    | None => Ok(()),
                }?;
            },
            | _ => {},
        };

        Ok(())
    }
}

#[derive(Debug)]
pub(crate) enum ManualFormat {
    Manpages,
    Markdown,
}

#[derive(Debug)]
pub enum StdoutFormat {
    #[cfg(feature = "format+json")]
    Json,
    #[cfg(feature = "format+yaml")]
    Yaml,
}

impl StdoutFormat {
    pub fn args() -> Vec<&'static str> {
        let mut args = Vec::<_>::new();
        #[cfg(feature = "format+json")]
        args.push("json");
        #[cfg(feature = "format+yaml")]
        args.push("yaml");
        args
    }
}

#[derive(Debug)]
pub(crate) enum Command {
    Manual {
        path: String,
        format: ManualFormat,
    },
    Autocomplete {
        path: String,
        shell: clap_complete::Shell,
    },

    Multiplex {
        program: Vec<String>,
        stdout: Option<StdoutFormat>,
        stderr: usize,
        commands: Vec<String>,
        parallelism: Option<usize>,
    },
}

pub(crate) struct ClapArgumentLoader {}

impl ClapArgumentLoader {
    pub(crate) fn root_command() -> clap::Command {
        clap::Command::new("bobr")
            .version(env!("CARGO_PKG_VERSION"))
            .about("A command multiplexer.")
            .author("Alexander Weber (cchexcode) <alexanderh.weber@outlook.com>")
            .propagate_version(true)
            .subcommand_required(false)
            .args([
                clap::Arg::new("experimental")
                    .short('e')
                    .long("experimental")
                    .help("Enables experimental features.")
                    .num_args(0),
                clap::Arg::new("program")
                    .long("program")
                    .help("Defines the program used to execute the commands given.")
                    .default_value("/bin/sh -c"),
                clap::Arg::new("stderr")
                    .long("stderr")
                    .help("Defines the length of stderr to display.")
                    .default_value("3"),
                clap::Arg::new("stdout")
                    .long("stdout")
                    .help(
                        "Marks whether the stdout of the processes are captured and returned in a structured format \
                         to stdout.",
                    )
                    .value_parser(StdoutFormat::args()),
                clap::Arg::new("parallelism")
                    .long("parallelism")
                    .short('p')
                    .help("Set the maximum amount of (sub) processes that run in parallel."),
                clap::Arg::new("command")
                    .short('c')
                    .long("command")
                    .help("A command to be executed.")
                    .action(ArgAction::Append),
                clap::Arg::new("file")
                    .short('f')
                    .long("file")
                    .help(
                        "Define a commands file. The content will be split per line, which are then interpreted as \
                         individual commands.",
                    )
                    .action(ArgAction::Append),
            ])
            .subcommand(
                clap::Command::new("man")
                    .about("Renders the manual.")
                    .arg(clap::Arg::new("out").short('o').long("out").required(true))
                    .arg(
                        clap::Arg::new("format")
                            .short('f')
                            .long("format")
                            .value_parser(["manpages", "markdown"])
                            .required(true),
                    ),
            )
            .subcommand(
                clap::Command::new("autocomplete")
                    .about("Renders shell completion scripts.")
                    .arg(clap::Arg::new("out").short('o').long("out").required(true))
                    .arg(
                        clap::Arg::new("shell")
                            .short('s')
                            .long("shell")
                            .value_parser(["bash", "zsh", "fish", "elvish", "powershell"])
                            .required(true),
                    ),
            )
    }

    pub(crate) fn load() -> Result<CallArgs> {
        let command = Self::root_command().get_matches();

        let privileges = if command.get_flag("experimental") {
            Privilege::Experimental
        } else {
            Privilege::Normal
        };

        let cmd = if let Some(subc) = command.subcommand_matches("man") {
            Command::Manual {
                path: subc.get_one::<String>("out").unwrap().into(),
                format: match subc.get_one::<String>("format").unwrap().as_str() {
                    | "manpages" => ManualFormat::Manpages,
                    | "markdown" => ManualFormat::Markdown,
                    | _ => return Err(anyhow!("unknown format")),
                },
            }
        } else if let Some(subc) = command.subcommand_matches("autocomplete") {
            Command::Autocomplete {
                path: subc.get_one::<String>("out").unwrap().into(),
                shell: clap_complete::Shell::from_str(subc.get_one::<String>("shell").unwrap().as_str()).unwrap(),
            }
        } else {
            let mut commands = command
                .get_many::<String>("command")
                .unwrap_or_default()
                .cloned()
                .collect_vec();
            if let Some(files) = command.get_many::<String>("file") {
                for file in files {
                    let mut content = String::new();
                    std::fs::File::open(file)?.read_to_string(&mut content)?;

                    let mut filters = HashMap::<&str, Box<dyn Fn(&str) -> Result<Config>>>::new();
                    #[cfg(feature = "format+yaml")]
                    {
                        // meh...
                        filters.insert(".yml", Box::new(move |v| Ok(serde_yml::from_str::<Config>(&v)?)));
                        filters.insert(".yaml", Box::new(move |v| Ok(serde_yml::from_str::<Config>(&v)?)));
                    }
                    #[cfg(feature = "format+json")]
                    filters.insert(".json", Box::new(move |v| Ok(serde_json::from_str::<Config>(&v)?)));
                    #[cfg(feature = "format+toml")]
                    filters.insert(".toml", Box::new(move |v| Ok(toml::from_str::<Config>(&v)?)));

                    let mut config = Option::<Config>::None;
                    for (format, parser) in filters.iter() {
                        if file.ends_with(format) {
                            config = Some(parser(&content)?);
                            break;
                        }
                    }
                    // add error handling
                    let config = config.unwrap();

                    let mut cmds = config.commands.into_iter().map(|v| v.command).collect::<Vec<_>>();
                    commands.append(&mut cmds);
                }
            }

            let program = command
                .get_one::<String>("program")
                .unwrap()
                .split_whitespace()
                .into_iter()
                .map(|v| v.to_owned())
                .collect::<Vec<_>>();
            Command::Multiplex {
                program,
                stderr: command.get_one::<String>("stderr").unwrap().parse::<usize>()?,
                stdout: match command.get_one::<String>("stdout") {
                    | Some(v) => match v.as_ref() {
                        #[cfg(feature = "format+json")]
                        | "json" => Ok(Some(StdoutFormat::Json)),
                        #[cfg(feature = "format+yaml")]
                        | "yaml" => Ok(Some(StdoutFormat::Yaml)),
                        | _ => Err(anyhow!("unknown stdout format")),
                    },
                    | None => Ok(None),
                }?,
                commands,
                parallelism: match command.get_one::<String>("parallelism") {
                    | Some(v) => Some(v.parse::<usize>().unwrap()),
                    | None => None,
                },
            }
        };

        let callargs = CallArgs {
            privileges,
            command: cmd,
        };

        callargs.validate()?;
        Ok(callargs)
    }
}
