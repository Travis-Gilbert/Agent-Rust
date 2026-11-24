use std::process::ExitCode;

use agent_rust::cli::Cli;
use clap::{Parser, error::ErrorKind};
use serde_json::json;

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            let _ = error.print();
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            println!(
                "{}",
                json!({"protocol_version":1,"type":"error","error":{"code":"invalid_arguments","message":error.to_string()}})
            );
            return ExitCode::from(2);
        }
    };
    match cli.execute() {
        Ok(data) => {
            println!(
                "{}",
                json!({"protocol_version":1,"type":"result","data":data})
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            let exit_code = if error.code == "invalid_arguments" {
                2
            } else {
                1
            };
            println!(
                "{}",
                json!({"protocol_version":1,"type":"error","error":error})
            );
            ExitCode::from(exit_code)
        }
    }
}
