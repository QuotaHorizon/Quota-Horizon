use std::fs;
use std::io;
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use thiserror::Error;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::sleep;

const INPUT_LINE_LIMIT: usize = 1024 * 1024;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub schema_version: String,
    pub name: String,
    pub steps: Vec<ScenarioStep>,
}

impl Scenario {
    pub fn from_directory(directory: &Path) -> Result<Self, ScenarioError> {
        let bytes = fs::read(directory.join("scenario.json"))?;
        let scenario: Self = serde_json::from_slice(&bytes)?;
        if scenario.schema_version != "1.0" {
            return Err(ScenarioError::UnsupportedSchemaVersion);
        }
        if scenario.name.trim().is_empty() || scenario.steps.is_empty() {
            return Err(ScenarioError::InvalidScenario);
        }
        Ok(scenario)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioStep {
    pub expect_method: String,
    pub action: ScenarioAction,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScenarioAction {
    Respond {
        result: Value,
    },
    NotifyThenRespond {
        method: String,
        params: Value,
        result: Value,
    },
    RpcError {
        code: i64,
        message: String,
    },
    RawLine {
        line: String,
    },
    RawRepeatedLine {
        prefix: String,
        fragment: String,
        repeat: usize,
        suffix: String,
    },
    RawFragmentThenClose {
        fragment: String,
    },
    NoResponse {
        delay_ms: u64,
    },
    Close,
}

pub async fn run_scenario<R: AsyncBufRead + Unpin, W: AsyncWrite + Unpin>(
    scenario: &Scenario,
    mut input: R,
    output: &mut W,
) -> Result<(), ScenarioError> {
    for step in &scenario.steps {
        let mut line = String::new();
        let bytes_read = input.read_line(&mut line).await?;
        if bytes_read == 0 {
            return Err(ScenarioError::UnexpectedEndOfInput);
        }
        if line.len() > INPUT_LINE_LIMIT {
            return Err(ScenarioError::InputLineTooLarge);
        }

        let request: Value = serde_json::from_str(&line)?;
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .ok_or(ScenarioError::MissingMethod)?;
        if method != step.expect_method {
            return Err(ScenarioError::UnexpectedMethod {
                expected: step.expect_method.clone(),
                actual: truncate(method, 128),
            });
        }

        match &step.action {
            ScenarioAction::Respond { result } => {
                let id = request
                    .get("id")
                    .cloned()
                    .ok_or(ScenarioError::MissingRequestId)?;
                write_json_line(output, &json!({ "id": id, "result": result })).await?;
            }
            ScenarioAction::NotifyThenRespond {
                method,
                params,
                result,
            } => {
                let id = request
                    .get("id")
                    .cloned()
                    .ok_or(ScenarioError::MissingRequestId)?;
                write_json_line(output, &json!({ "method": method, "params": params })).await?;
                write_json_line(output, &json!({ "id": id, "result": result })).await?;
            }
            ScenarioAction::RpcError { code, message } => {
                let id = request
                    .get("id")
                    .cloned()
                    .ok_or(ScenarioError::MissingRequestId)?;
                write_json_line(
                    output,
                    &json!({
                        "id": id,
                        "error": {
                            "code": code,
                            "message": truncate(message, 256)
                        }
                    }),
                )
                .await?;
            }
            ScenarioAction::RawLine { line } => {
                output.write_all(line.as_bytes()).await?;
                output.write_all(b"\n").await?;
                output.flush().await?;
            }
            ScenarioAction::RawRepeatedLine {
                prefix,
                fragment,
                repeat,
                suffix,
            } => {
                if *repeat > INPUT_LINE_LIMIT * 4
                    || prefix.len() > INPUT_LINE_LIMIT
                    || fragment.len() > INPUT_LINE_LIMIT
                    || suffix.len() > INPUT_LINE_LIMIT
                {
                    return Err(ScenarioError::InvalidScenario);
                }
                output.write_all(prefix.as_bytes()).await?;
                for _ in 0..*repeat {
                    output.write_all(fragment.as_bytes()).await?;
                }
                output.write_all(suffix.as_bytes()).await?;
                output.write_all(b"\n").await?;
                output.flush().await?;
            }
            ScenarioAction::RawFragmentThenClose { fragment } => {
                output.write_all(fragment.as_bytes()).await?;
                output.flush().await?;
                return Ok(());
            }
            ScenarioAction::NoResponse { delay_ms } => {
                sleep(Duration::from_millis(*delay_ms)).await;
            }
            ScenarioAction::Close => return Ok(()),
        }
    }
    Ok(())
}

async fn write_json_line(
    output: &mut (impl AsyncWrite + Unpin),
    value: &Value,
) -> Result<(), ScenarioError> {
    let mut line = serde_json::to_vec(value)?;
    line.push(b'\n');
    output.write_all(&line).await?;
    output.flush().await?;
    Ok(())
}

fn truncate(value: &str, maximum_chars: usize) -> String {
    value.chars().take(maximum_chars).collect()
}

#[derive(Debug, Error)]
pub enum ScenarioError {
    #[error("scenario I/O failed")]
    Io(#[from] io::Error),
    #[error("scenario JSON is invalid")]
    Json(#[from] serde_json::Error),
    #[error("scenario schema version is unsupported")]
    UnsupportedSchemaVersion,
    #[error("scenario must have a name and at least one step")]
    InvalidScenario,
    #[error("client input ended before the scenario completed")]
    UnexpectedEndOfInput,
    #[error("client input line exceeded 1 MiB")]
    InputLineTooLarge,
    #[error("client message did not include a method")]
    MissingMethod,
    #[error("request action requires a request ID")]
    MissingRequestId,
    #[error("expected method {expected}, received {actual}")]
    UnexpectedMethod { expected: String, actual: String },
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    #[tokio::test]
    async fn response_reuses_the_clients_request_id() {
        let scenario = Scenario {
            schema_version: "1.0".to_owned(),
            name: "test".to_owned(),
            steps: vec![ScenarioStep {
                expect_method: "account/read".to_owned(),
                action: ScenarioAction::Respond {
                    result: json!({ "account": null, "requiresOpenaiAuth": true }),
                },
            }],
        };
        let input = Cursor::new(b"{\"id\":\"request-42\",\"method\":\"account/read\"}\n");
        let mut output = Vec::new();

        run_scenario(&scenario, input, &mut output).await.unwrap();

        let response: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(response["id"], "request-42");
        assert_eq!(response["result"]["requiresOpenaiAuth"], true);
    }

    #[tokio::test]
    async fn unexpected_method_does_not_echo_the_request_payload() {
        let scenario = Scenario {
            schema_version: "1.0".to_owned(),
            name: "test".to_owned(),
            steps: vec![ScenarioStep {
                expect_method: "initialize".to_owned(),
                action: ScenarioAction::Close,
            }],
        };
        let input =
            Cursor::new(b"{\"id\":1,\"method\":\"wrong\",\"secret\":\"never-echo-this\"}\n");
        let mut output = Vec::new();

        let error = run_scenario(&scenario, input, &mut output)
            .await
            .unwrap_err();

        assert!(!error.to_string().contains("never-echo-this"));
    }

    #[tokio::test]
    async fn repeated_line_action_generates_bounded_fixture_output() {
        let scenario = Scenario {
            schema_version: "1.0".to_owned(),
            name: "test".to_owned(),
            steps: vec![ScenarioStep {
                expect_method: "initialize".to_owned(),
                action: ScenarioAction::RawRepeatedLine {
                    prefix: "{\"padding\":\"".to_owned(),
                    fragment: "x".to_owned(),
                    repeat: 4,
                    suffix: "\"}".to_owned(),
                },
            }],
        };
        let input = Cursor::new(b"{\"id\":0,\"method\":\"initialize\"}\n");
        let mut output = Vec::new();

        run_scenario(&scenario, input, &mut output).await.unwrap();

        assert_eq!(output, b"{\"padding\":\"xxxx\"}\n");
    }
}
