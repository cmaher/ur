use serde::Serialize;

#[derive(Serialize)]
pub struct CommandSchema {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub args: Vec<ArgSchema>,
    pub subcommands: Vec<CommandSchema>,
}

#[derive(Serialize)]
pub struct ArgSchema {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub required: bool,
    pub arg_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub possible_values: Option<Vec<String>>,
}

pub fn describe_command(cmd: &clap::Command) -> CommandSchema {
    let args: Vec<ArgSchema> = cmd
        .get_arguments()
        .filter(|a| a.get_id() != "help" && a.get_id() != "version")
        .map(|a| {
            let possible = a.get_possible_values();
            let possible_values = if possible.is_empty() {
                None
            } else {
                Some(
                    possible
                        .iter()
                        .map(|v| v.get_name().to_string())
                        .collect(),
                )
            };

            let default_value = a
                .get_default_values()
                .first()
                .map(|v| v.to_string_lossy().into_owned());

            let arg_type = if a.get_action().takes_values() {
                "string".to_string()
            } else {
                "bool".to_string()
            };

            ArgSchema {
                name: a.get_id().to_string(),
                description: a.get_help().map(|h| h.to_string()),
                required: a.is_required_set(),
                arg_type,
                default_value,
                possible_values,
            }
        })
        .collect();

    let subcommands: Vec<CommandSchema> = cmd
        .get_subcommands()
        .filter(|s| s.get_name() != "help")
        .map(describe_command)
        .collect();

    CommandSchema {
        name: cmd.get_name().to_string(),
        description: cmd.get_about().map(|a| a.to_string()),
        args,
        subcommands,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{Arg, Command};

    #[test]
    fn describe_simple_command() {
        let cmd = Command::new("test")
            .about("A test command")
            .arg(Arg::new("name").required(true).help("The name"));
        let schema = describe_command(&cmd);
        assert_eq!(schema.name, "test");
        assert_eq!(schema.description.as_deref(), Some("A test command"));
        assert_eq!(schema.args.len(), 1);
        assert_eq!(schema.args[0].name, "name");
        assert!(schema.args[0].required);
    }

    #[test]
    fn describe_with_subcommands() {
        let cmd = Command::new("root")
            .subcommand(Command::new("sub1").about("First sub"))
            .subcommand(Command::new("sub2").about("Second sub"));
        let schema = describe_command(&cmd);
        assert_eq!(schema.subcommands.len(), 2);
        assert_eq!(schema.subcommands[0].name, "sub1");
        assert_eq!(schema.subcommands[1].name, "sub2");
    }
}
