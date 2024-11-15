use {
    anyhow::{
        anyhow,
        Result,
    },
    clap::ArgAction,
    itertools::Itertools,
    std::{
        io::Read,
        str::FromStr,
    },
};

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
            | _ => (),
        }

        Ok(())
    }
}

#[derive(Debug)]
pub(crate) enum ManualFormat {
    Manpages,
    Markdown,
}

#[derive(Debug)]
pub(crate) enum Command {
    Manual { path: String, format: ManualFormat },
    Autocomplete { path: String, shell: clap_complete::Shell },

    Multiplex { commands: Vec<String> },
}

pub(crate) struct ClapArgumentLoader {}

impl ClapArgumentLoader {
    pub(crate) fn root_command() -> clap::Command {
        clap::Command::new("bobr")
            .version(env!("CARGO_PKG_VERSION"))
            .about("A command multiplexer.")
            .author("replicadse <aw@voidpointergroup.com>")
            .propagate_version(true)
            .subcommand_required(false)
            .args([
                clap::Arg::new("experimental")
                    .short('e')
                    .long("experimental")
                    .help("Enables experimental features.")
                    .num_args(0),
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
                    let lines = &mut content.lines().map(|v| v.to_owned()).collect::<Vec<_>>();
                    commands.append(lines);
                }
            }
            Command::Multiplex { commands }
        };

        let callargs = CallArgs {
            privileges,
            command: cmd,
        };

        callargs.validate()?;
        Ok(callargs)
    }
}
