//! `--help-json`: the CLI's own shape as JSON.
//!
//! `--help` is prose for a person; an agent probing an unfamiliar binary
//! wants the same facts as data — every subcommand, every flag, which
//! ones are required, what values they take, and what they conflict
//! with. This walks clap's `Command` tree, so it can never drift from the
//! parser the way a hand-maintained description would.

use clap::{Arg, ArgAction, Command};
use serde_json::{Map, Value, json};

/// The flag that triggers this output. Checked *before* clap parses, so
/// `colab refactor --help-json` works even though `refactor` has
/// required arguments.
pub const FLAG: &str = "--help-json";

/// If `argv` asks for `--help-json`, the JSON for the subcommand it names
/// (or the root). `None` means "parse normally".
pub fn requested(root: &Command, argv: &[String]) -> Option<Value> {
    if !argv.iter().skip(1).any(|a| a == FLAG) {
        return None;
    }
    // Follow leading non-flag words down the subcommand tree:
    // `colab pack list --help-json` describes `pack list`.
    let mut command = root;
    let mut path = vec!["colab".to_string()];
    for word in argv.iter().skip(1).take_while(|a| !a.starts_with('-')) {
        match command.find_subcommand(word) {
            Some(sub) => {
                command = sub;
                path.push(sub.get_name().to_string());
            }
            None => break,
        }
    }
    let mut value = describe(command);
    if path.len() == 1 {
        value["name"] = json!("colab");
    }
    value["path"] = json!(path);
    Some(value)
}

fn describe(command: &Command) -> Value {
    let mut out = Map::new();
    out.insert("name".into(), json!(command.get_name()));
    if let Some(about) = command.get_about() {
        out.insert("about".into(), json!(about.to_string()));
    }
    let args: Vec<Value> = command
        .get_arguments()
        .filter(|a| {
            !a.is_hide_set() && !matches!(a.get_id().as_str(), "help" | "version" | "help_json")
        })
        .map(|a| describe_arg(command, a))
        .collect();
    if !args.is_empty() {
        out.insert("args".into(), Value::Array(args));
    }
    let groups: Vec<Value> = command
        .get_groups()
        .filter(|g| g.is_required_set())
        .map(|g| {
            json!({
                "one_of": g.get_args().map(|id| id.as_str()).collect::<Vec<_>>(),
                "multiple": g.clone().is_multiple(),
            })
        })
        .collect();
    if !groups.is_empty() {
        out.insert("required_groups".into(), Value::Array(groups));
    }
    let subcommands: Vec<Value> = command
        .get_subcommands()
        .filter(|s| s.get_name() != "help")
        .map(describe)
        .collect();
    if !subcommands.is_empty() {
        out.insert("subcommands".into(), Value::Array(subcommands));
    }
    Value::Object(out)
}

fn describe_arg(command: &Command, arg: &Arg) -> Value {
    let mut out = Map::new();
    out.insert("id".into(), json!(arg.get_id().as_str()));
    if let Some(long) = arg.get_long() {
        out.insert("long".into(), json!(format!("--{long}")));
    }
    if let Some(short) = arg.get_short() {
        out.insert("short".into(), json!(format!("-{short}")));
    }
    if arg.is_positional() {
        out.insert("positional".into(), json!(true));
    }
    let takes_value = !matches!(
        arg.get_action(),
        ArgAction::SetTrue | ArgAction::SetFalse | ArgAction::Count
    );
    out.insert("takes_value".into(), json!(takes_value));
    if matches!(arg.get_action(), ArgAction::Append)
        || arg.get_num_args().is_some_and(|n| n.max_values() > 1)
    {
        out.insert("repeatable".into(), json!(true));
    }
    if arg.is_required_set() {
        out.insert("required".into(), json!(true));
    }
    if let Some(names) = arg.get_value_names().filter(|_| takes_value) {
        out.insert(
            "value_name".into(),
            json!(
                names
                    .iter()
                    .map(|n| n.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
        );
    }
    let possible: Vec<String> = arg
        .get_possible_values()
        .iter()
        .filter(|v| !v.is_hide_set())
        .map(|v| v.get_name().to_string())
        .collect();
    if !possible.is_empty() && takes_value {
        out.insert("values".into(), json!(possible));
    }
    let defaults: Vec<String> = arg
        .get_default_values()
        .iter()
        .map(|v| v.to_string_lossy().into_owned())
        .collect();
    if !defaults.is_empty() {
        out.insert("default".into(), json!(defaults.join(",")));
    }
    let conflicts: Vec<String> = command
        .get_arg_conflicts_with(arg)
        .iter()
        .map(|a| a.get_id().to_string())
        .collect();
    if !conflicts.is_empty() {
        out.insert("conflicts_with".into(), json!(conflicts));
    }
    if let Some(help) = arg.get_help() {
        out.insert("help".into(), json!(help.to_string()));
    }
    Value::Object(out)
}
