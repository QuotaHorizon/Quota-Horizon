use std::collections::HashSet;
use std::fmt;
use std::time::Duration;

use serde::de::{Error as _, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Number, Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::timeout;

use super::{RuntimeConfig, RuntimeError};
use crate::app_server::model::NotificationSummary;

pub(super) struct JsonlSession<R, W> {
    reader: R,
    writer: W,
    config: RuntimeConfig,
    next_request_id: u64,
    completed_ids: HashSet<u64>,
    stdout_bytes: usize,
    notifications: NotificationSummary,
    rate_limit_refresh_pending: bool,
    account_change_pending: bool,
    pub(super) account_baseline_established: bool,
}

impl<R, W> JsonlSession<R, W>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    pub(super) fn new(reader: R, writer: W, config: RuntimeConfig) -> Self {
        Self {
            reader,
            writer,
            config,
            next_request_id: 0,
            completed_ids: HashSet::new(),
            stdout_bytes: 0,
            notifications: NotificationSummary::default(),
            rate_limit_refresh_pending: false,
            account_change_pending: false,
            account_baseline_established: false,
        }
    }

    pub(super) async fn request(
        &mut self,
        method: &'static str,
        params: Option<Value>,
        request_timeout: Duration,
    ) -> Result<Value, RuntimeError> {
        let id = self.next_request_id;
        self.next_request_id = self
            .next_request_id
            .checked_add(1)
            .ok_or(RuntimeError::RequestIdExhausted)?;

        let message = match params {
            Some(params) => json!({ "method": method, "id": id, "params": params }),
            None => json!({ "method": method, "id": id }),
        };
        self.write_message(&message).await?;

        timeout(request_timeout, self.wait_for_response(id, method))
            .await
            .map_err(|_| RuntimeError::Timeout { phase: method })?
    }

    pub(super) async fn notify(
        &mut self,
        method: &'static str,
        params: Value,
    ) -> Result<(), RuntimeError> {
        self.write_message(&json!({ "method": method, "params": params }))
            .await
    }

    pub(super) fn take_rate_limit_refresh_pending(&mut self) -> bool {
        std::mem::take(&mut self.rate_limit_refresh_pending)
    }

    pub(super) fn take_notifications(&mut self) -> NotificationSummary {
        std::mem::take(&mut self.notifications)
    }

    pub(super) fn take_account_change_pending(&mut self) -> bool {
        std::mem::take(&mut self.account_change_pending)
    }

    pub(super) fn reset_stdout_budget(&mut self) {
        self.stdout_bytes = 0;
    }

    async fn write_message(&mut self, value: &Value) -> Result<(), RuntimeError> {
        let mut bytes = serde_json::to_vec(value).map_err(|_| RuntimeError::Encode)?;
        if bytes.len() > self.config.max_jsonl_line_bytes {
            return Err(RuntimeError::OutboundLineTooLarge);
        }
        bytes.push(b'\n');
        self.writer
            .write_all(&bytes)
            .await
            .map_err(|_| RuntimeError::Write)?;
        self.writer.flush().await.map_err(|_| RuntimeError::Write)
    }

    async fn wait_for_response(
        &mut self,
        expected_id: u64,
        method: &'static str,
    ) -> Result<Value, RuntimeError> {
        loop {
            let line = self.read_bounded_line().await?;
            let value = parse_unique_json(&line)?;
            let object = value.as_object().ok_or(RuntimeError::InvalidMessage)?;
            let id = object.get("id");
            let incoming_method = object.get("method");

            match (id, incoming_method) {
                (None, Some(incoming_method)) => {
                    self.record_notification(object, incoming_method)?;
                }
                (Some(_), Some(_)) => return Err(RuntimeError::ServerRequestRejected),
                (Some(id), None) => {
                    let id = id.as_u64().ok_or(RuntimeError::InvalidResponseId)?;
                    if id != expected_id {
                        return if self.completed_ids.contains(&id) {
                            Err(RuntimeError::DuplicateResponseId)
                        } else {
                            Err(RuntimeError::MismatchedResponseId)
                        };
                    }

                    let result = object.get("result");
                    let error = object.get("error");
                    if result.is_some() == error.is_some() {
                        return Err(RuntimeError::InvalidMessage);
                    }
                    self.completed_ids.insert(id);

                    if let Some(result) = result {
                        return Ok(result.clone());
                    }

                    let error = error
                        .and_then(Value::as_object)
                        .ok_or(RuntimeError::InvalidMessage)?;
                    let code = error
                        .get("code")
                        .and_then(Value::as_i64)
                        .ok_or(RuntimeError::InvalidMessage)?;
                    return Err(RuntimeError::Rpc { method, code });
                }
                (None, None) => return Err(RuntimeError::InvalidMessage),
            }
        }
    }

    fn record_notification(
        &mut self,
        object: &Map<String, Value>,
        method: &Value,
    ) -> Result<(), RuntimeError> {
        let method = method.as_str().ok_or(RuntimeError::InvalidMessage)?;
        if method.is_empty()
            || method.len() > 128
            || method.chars().any(|character| character.is_control())
        {
            return Err(RuntimeError::InvalidMessage);
        }

        if method == "account/rateLimits/updated" {
            let valid_shape = object
                .get("params")
                .and_then(Value::as_object)
                .and_then(|params| params.get("rateLimits"))
                .is_some_and(Value::is_object);
            if !valid_shape {
                return Err(RuntimeError::InvalidMessage);
            }
            self.notifications.rate_limit_updates =
                self.notifications.rate_limit_updates.saturating_add(1);
            self.rate_limit_refresh_pending = true;
        } else if method == "account/updated" {
            let params = object
                .get("params")
                .and_then(Value::as_object)
                .ok_or(RuntimeError::InvalidAccountNotification)?;
            for field in ["authMode", "planType"] {
                if let Some(value) = params.get(field) {
                    let valid = value.is_null()
                        || value.as_str().is_some_and(|value| {
                            !value.is_empty()
                                && value.len() <= 64
                                && !value.chars().any(char::is_control)
                        });
                    if !valid {
                        return Err(RuntimeError::InvalidAccountNotification);
                    }
                }
            }
            self.notifications.account_updates =
                self.notifications.account_updates.saturating_add(1);
            self.account_change_pending = true;
        } else {
            self.notifications.unknown_notifications =
                self.notifications.unknown_notifications.saturating_add(1);
        }
        Ok(())
    }

    async fn read_bounded_line(&mut self) -> Result<Vec<u8>, RuntimeError> {
        let mut line = Vec::new();

        loop {
            let (consumed, found_newline) = {
                let available = self
                    .reader
                    .fill_buf()
                    .await
                    .map_err(|_| RuntimeError::Read)?;
                if available.is_empty() {
                    return if line.is_empty() {
                        Err(RuntimeError::UnexpectedEof)
                    } else {
                        Err(RuntimeError::PartialLineEof)
                    };
                }

                match available.iter().position(|byte| *byte == b'\n') {
                    Some(position) => {
                        if line.len().saturating_add(position) > self.config.max_jsonl_line_bytes {
                            return Err(RuntimeError::InboundLineTooLarge);
                        }
                        line.extend_from_slice(&available[..position]);
                        (position + 1, true)
                    }
                    None => {
                        if line.len().saturating_add(available.len())
                            > self.config.max_jsonl_line_bytes
                        {
                            return Err(RuntimeError::InboundLineTooLarge);
                        }
                        line.extend_from_slice(available);
                        (available.len(), false)
                    }
                }
            };

            self.reader.consume(consumed);
            self.stdout_bytes = self.stdout_bytes.saturating_add(consumed);
            if self.stdout_bytes > self.config.max_stdout_bytes {
                return Err(RuntimeError::StdoutLimitExceeded);
            }

            if found_newline {
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                if line.is_empty() {
                    return Err(RuntimeError::InvalidMessage);
                }
                return Ok(line);
            }
        }
    }
}

fn parse_unique_json(bytes: &[u8]) -> Result<Value, RuntimeError> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = UniqueValue::deserialize(&mut deserializer)
        .map_err(|_| RuntimeError::MalformedJson)?
        .0;
    deserializer
        .end()
        .map_err(|_| RuntimeError::MalformedJson)?;
    Ok(value)
}

struct UniqueValue(Value);

impl<'de> Deserialize<'de> for UniqueValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(UniqueValueVisitor)
    }
}

struct UniqueValueVisitor;

impl<'de> Visitor<'de> for UniqueValueVisitor {
    type Value = UniqueValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value without duplicate object keys")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Number(Number::from(value))))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Number(Number::from(value))))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Number::from_f64(value)
            .map(Value::Number)
            .map(UniqueValue)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(UniqueValue(Value::String(value.to_owned())))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::String(value)))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Null))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Null))
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element::<UniqueValue>()? {
            values.push(value.0);
        }
        Ok(UniqueValue(Value::Array(values)))
    }

    fn visit_map<A>(self, mut entries: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = Map::new();
        while let Some((key, value)) = entries.next_entry::<String, UniqueValue>()? {
            if values.contains_key(&key) {
                return Err(A::Error::custom("duplicate object key"));
            }
            values.insert(key, value.0);
        }
        Ok(UniqueValue(Value::Object(values)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{BufReader, duplex, split};

    #[tokio::test]
    async fn duplicate_json_keys_fail_closed() {
        let (client, mut server) = duplex(256);
        tokio::spawn(async move {
            server
                .write_all(b"{\"id\":0,\"id\":0,\"result\":{}}\n")
                .await
                .unwrap();
        });
        let (reader, writer) = split(client);
        let mut session =
            JsonlSession::new(BufReader::new(reader), writer, RuntimeConfig::default());

        let error = session
            .request("initialize", Some(json!({})), Duration::from_secs(1))
            .await
            .unwrap_err();

        assert!(matches!(error, RuntimeError::MalformedJson));
    }
}
