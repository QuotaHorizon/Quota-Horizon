fn reliability_chat_stream(events: &[Value], tail: &str) -> String {
    let mut source = events
        .iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect::<String>();
    source.push_str(tail);
    let mut reader = ChatSseReader::new(
        BufReader::new(Cursor::new(source.into_bytes())),
        "test-model".to_string(),
        CodexToolContext::default(),
        None,
    );
    let mut bytes = Vec::new();
    // Exercise event boundaries that cross downstream read buffers.
    let mut chunk = [0_u8; 7];
    loop {
        let count = reader.read(&mut chunk).unwrap();
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    assert_eq!(reader.read(&mut chunk).unwrap(), 0);
    String::from_utf8(bytes).unwrap()
}

fn assert_reliability_terminal(output: &str, terminal: &str) {
    for candidate in [
        "response.completed",
        "response.failed",
        "response.incomplete",
    ] {
        assert_eq!(
            output.matches(&format!("event: {candidate}\n")).count(),
            usize::from(candidate == terminal),
            "{output}"
        );
    }
    assert_eq!(output.matches("data: [DONE]").count(), 1, "{output}");
}

#[test]
fn reliability_stream_accepts_explicit_finish_at_eof_with_trailing_usage() {
    let output = reliability_chat_stream(
        &[
            json!({ "choices": [{ "delta": { "content": "Hello" } }] }),
            json!({ "choices": [{ "delta": {}, "finish_reason": "stop" }] }),
            json!({ "choices": [], "usage": { "prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7 } }),
        ],
        "",
    );
    assert_reliability_terminal(&output, "response.completed");
    let completed = sse_event(&output, "response.completed");
    assert_eq!(completed["response"]["usage"]["total_tokens"], 7);
    assert_eq!(
        completed["response"]["output"][0]["content"][0]["text"],
        "Hello"
    );
}

#[test]
fn reliability_stream_accepts_done_marker_and_ignores_duplicate_terminal() {
    let output = reliability_chat_stream(
        &[json!({ "choices": [{ "delta": { "content": "Hello" } }] })],
        "data: [DONE]\n\ndata: [DONE]\n\ndata: {\"error\":{\"message\":\"late\"}}\n\n",
    );
    assert_reliability_terminal(&output, "response.completed");
}

#[test]
fn reliability_stream_eof_preserves_partial_text_without_success() {
    let output = reliability_chat_stream(
        &[
            json!({ "choices": [{ "delta": { "reasoning_content": "Thinking" } }] }),
            json!({ "choices": [{ "delta": { "content": "Partial answer" } }] }),
        ],
        "",
    );
    assert_reliability_terminal(&output, "response.failed");
    let failed = sse_event(&output, "response.failed");
    assert_eq!(
        failed["response"]["output"][0]["summary"][0]["text"],
        "Thinking"
    );
    assert_eq!(
        failed["response"]["output"][1]["content"][0]["text"],
        "Partial answer"
    );
    assert!(!output.contains("event: response.output_text.done"));
}

#[test]
fn reliability_stream_maps_length_and_filter_to_incomplete() {
    for (reason, expected) in [
        ("length", "max_output_tokens"),
        ("content_filter", "content_filter"),
    ] {
        let output = reliability_chat_stream(
            &[
                json!({ "choices": [{ "delta": { "content": "Partial" }, "finish_reason": reason }] }),
                json!({ "choices": [], "usage": { "prompt_tokens": 5, "completion_tokens": 2 } }),
            ],
            "data: [DONE]\n\n",
        );
        assert_reliability_terminal(&output, "response.incomplete");
        let incomplete = sse_event(&output, "response.incomplete");
        assert_eq!(
            incomplete["response"]["incomplete_details"]["reason"],
            expected
        );
        assert_eq!(
            incomplete["response"]["output"][0]["content"][0]["text"],
            "Partial"
        );
        assert_eq!(incomplete["response"]["usage"]["output_tokens"], 2);
    }
}

#[test]
fn reliability_stream_handles_in_band_error_and_malformed_events() {
    for tail in [
        "data: {\"error\":{\"message\":\"Upstream overloaded\"}}\n\ndata: [DONE]\n\n",
        "event: error\ndata: {\"message\":\"Upstream overloaded\"}\n\n",
        "data: {broken-json}\n\n",
        "data: {\"choices\":[{\"finish_reason\":\"unexpected\"}]}\n\n",
    ] {
        let output = reliability_chat_stream(
            &[json!({ "choices": [{ "delta": { "content": "Already received" } }] })],
            tail,
        );
        assert_reliability_terminal(&output, "response.failed");
        assert_eq!(
            sse_event(&output, "response.failed")["response"]["output"][0]["content"][0]["text"],
            "Already received"
        );
    }
}

#[test]
fn reliability_stream_transport_error_keeps_received_content() {
    struct FailingRead(Cursor<Vec<u8>>);
    impl Read for FailingRead {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let count = self.0.read(buffer)?;
            if count == 0 {
                Err(io::Error::new(io::ErrorKind::ConnectionReset, "mock reset"))
            } else {
                Ok(count)
            }
        }
    }
    let source = b"data: {\"choices\":[{\"delta\":{\"content\":\"Partial\"}}]}\n\n";
    let mut reader = ChatSseReader::new(
        BufReader::new(FailingRead(Cursor::new(source.to_vec()))),
        "test-model".to_string(),
        CodexToolContext::default(),
        None,
    );
    let mut output = String::new();
    reader.read_to_string(&mut output).unwrap();
    assert_reliability_terminal(&output, "response.failed");
    assert_eq!(
        sse_event(&output, "response.failed")["response"]["output"][0]["content"][0]["text"],
        "Partial"
    );
}

#[test]
fn reliability_stream_never_completes_partial_tool_arguments() {
    for (arguments, finish_reason, tail, terminal) in [
        ("{\"path\":", Value::Null, "", "response.failed"),
        (
            "{\"path\":",
            json!("tool_calls"),
            "data: [DONE]\n\n",
            "response.failed",
        ),
        (
            "{\"path\":",
            json!("length"),
            "data: [DONE]\n\n",
            "response.incomplete",
        ),
    ] {
        let output = reliability_chat_stream(
            &[
                json!({ "choices": [{ "delta": { "tool_calls": [{ "index": 0, "id": "call-partial", "function": { "name": "write_file", "arguments": arguments } }] }, "finish_reason": finish_reason }] }),
            ],
            tail,
        );
        assert_reliability_terminal(&output, terminal);
        assert!(!output.contains("event: response.function_call_arguments.done"));
        assert!(!output.contains("event: response.output_item.done"));
        let item = &sse_event(&output, terminal)["response"]["output"][0];
        assert_eq!(item["status"], "incomplete");
        assert_eq!(item["arguments"], arguments);
    }
}

#[test]
fn reliability_stream_finishes_valid_tool_call_once() {
    let output = reliability_chat_stream(
        &[
            json!({ "choices": [{ "delta": { "tool_calls": [{ "index": 0, "id": "call-valid", "function": { "name": "read_file", "arguments": "{\"path\":" } }] } }] }),
            json!({ "choices": [{ "delta": { "tool_calls": [{ "index": 0, "function": { "arguments": "\"file\"}" } }] }, "finish_reason": "tool_calls" }] }),
        ],
        "",
    );
    assert_reliability_terminal(&output, "response.completed");
    assert_eq!(
        output
            .matches("event: response.function_call_arguments.done\n")
            .count(),
        1
    );
    assert_eq!(
        sse_event(&output, "response.function_call_arguments.done")["arguments"],
        "{\"path\":\"file\"}"
    );
}

#[test]
fn reliability_truncated_tool_call_is_not_cached_for_continuation() {
    let scope =
        chat_bridge_continuation::ContinuationScope::new("test", &uuid::Uuid::new_v4().to_string());
    let source = concat!(
        "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"Not complete\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call\",\"function\":{\"name\":\"run\",\"arguments\":\"{\"}}]}}]}\n\n"
    );
    let mut reader = ChatSseReader::new(
        BufReader::new(Cursor::new(source)),
        "test-model".to_string(),
        CodexToolContext::default(),
        Some(scope.clone()),
    );
    let mut output = String::new();
    reader.read_to_string(&mut output).unwrap();
    assert_reliability_terminal(&output, "response.failed");
    let mut messages = vec![
        json!({ "role": "assistant", "tool_calls": [{ "id": "call", "type": "function", "function": { "name": "run", "arguments": "{}" } }] }),
    ];
    chat_bridge_continuation::restore_messages(&scope, &mut messages);
    assert!(messages[0].get("reasoning_content").is_none());
}

#[test]
fn reliability_buffered_stream_conversion_does_not_complete_truncated_input() {
    let output = chat_sse_to_responses_sse(
        "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"Partial\"}}]}\n\n",
        "test-model",
    );
    assert_reliability_terminal(&output, "response.failed");
}

#[test]
fn reliability_anthropic_bridge_preserves_failures_and_partial_output() {
    let source = reliability_chat_stream(
        &[
            json!({ "choices": [{ "delta": { "content": "Partial answer" } }] }),
            json!({ "choices": [{ "delta": { "tool_calls": [{ "index": 0, "id": "call-partial", "function": { "name": "run", "arguments": "{\"cmd\":" } }] } }] }),
        ],
        "",
    );
    let converted = responses_sse_to_anthropic(source.as_bytes(), "claude-test");
    assert_eq!(
        sse_event(&converted, "content_block_delta")["delta"]["text"],
        "Partial answer"
    );
    assert!(converted.contains("partial_json"));
    assert_eq!(converted.matches("event: error\n").count(), 1);
    assert!(!converted.contains("event: message_stop"));
    assert!(!converted.contains("event: message_delta"));
    let payload = UpstreamPayload {
        status: 200,
        content_type: Some("text/event-stream".to_string()),
        response_headers: Vec::new(),
        body: UpstreamBody::Buffered(source.into_bytes()),
        token_usage_account: None,
    };
    let converted = convert_responses_payload(payload, false, "claude-test").unwrap();
    assert_eq!(converted.status, 502);
}

#[test]
fn reliability_anthropic_bridge_reports_token_limit_without_tool_use_completion() {
    let source = reliability_chat_stream(
        &[json!({ "choices": [{ "delta": { "content": "Partial" }, "finish_reason": "length" }] })],
        "data: [DONE]\n\n",
    );
    let converted = responses_sse_to_anthropic(source.as_bytes(), "claude-test");
    assert_eq!(
        sse_event(&converted, "message_delta")["delta"]["stop_reason"],
        "max_tokens"
    );
    assert_eq!(converted.matches("event: message_stop\n").count(), 1);
    let message = responses_sse_to_anthropic_message(source.as_bytes(), "claude-test");
    assert_eq!(message["stop_reason"], "max_tokens");
    assert_eq!(message["content"][0]["text"], "Partial");
}

#[test]
fn reliability_anthropic_token_limit_cannot_finalize_partial_tool_input() {
    let source = reliability_chat_stream(
        &[
            json!({ "choices": [{ "delta": { "tool_calls": [{ "index": 0, "id": "call-partial", "function": { "name": "run", "arguments": "{\"cmd\":" } }] }, "finish_reason": "length" }] }),
        ],
        "data: [DONE]\n\n",
    );
    let converted = responses_sse_to_anthropic(source.as_bytes(), "claude-test");
    assert_eq!(
        sse_event(&converted, "content_block_delta")["delta"]["partial_json"],
        "{\"cmd\":"
    );
    assert_eq!(converted.matches("event: error\n").count(), 1);
    assert!(!converted.contains("event: content_block_stop"));
    assert!(!converted.contains("event: message_delta"));
    assert!(!converted.contains("event: message_stop"));
    assert_eq!(
        responses_sse_to_anthropic_message(source.as_bytes(), "claude-test")["type"],
        "error"
    );
}

#[test]
fn reliability_anthropic_stream_handles_crlf_and_rejects_malformed_frames() {
    let source = reliability_chat_stream(
        &[json!({ "choices": [{ "delta": { "content": "Complete" }, "finish_reason": "stop" }] })],
        "data: [DONE]\n\n",
    );
    let converted =
        responses_sse_to_anthropic(source.replace('\n', "\r\n").as_bytes(), "claude-test");
    assert_eq!(
        sse_event(&converted, "message_delta")["delta"]["stop_reason"],
        "end_turn"
    );
    assert_eq!(converted.matches("event: message_stop\n").count(), 1);
    let malformed = format!("data: {{broken}}\n\n{source}");
    let converted = responses_sse_to_anthropic(malformed.as_bytes(), "claude-test");
    assert_eq!(converted.matches("event: error\n").count(), 1);
    assert!(!converted.contains("event: message_stop"));
}

#[test]
fn reliability_empty_sse_heartbeat_does_not_fail_valid_completion() {
    let output = reliability_chat_stream(&[],
        "data: \r\n\r\n: keepalive\r\n\r\ndata: {\"choices\":[{\"delta\":{\"content\":\"Done\"},\"finish_reason\":\"stop\"}]}");
    assert_reliability_terminal(&output, "response.completed");
    assert_eq!(
        sse_event(&output, "response.completed")["response"]["output"][0]["content"][0]["text"],
        "Done"
    );
}

#[test]
fn reliability_quota_classification_requires_exact_structured_codes() {
    for status in [403, 429] {
        for code in [
            "insufficient_quota",
            "usage_limit_reached",
            "quota_exhausted",
            "quota_exceeded",
            "billing_hard_limit_reached",
        ] {
            assert!(is_official_quota_exhaustion(&json_payload(
                status,
                json!({ "error": { "code": code } })
            )));
        }
        for value in [
            json!({ "error": { "code": "rate_limit_exceeded", "message": "Quota limit reached, try again" } }),
            json!({ "error": { "type": "rate_limit_error" } }),
            json!({ "error": { "message": "usage_limit_reached quota 额度 exhausted" } }),
            json!({ "error": "insufficient_quota" }),
            json!({ "error": { "code": "unknown_quota" } }),
        ] {
            assert!(!is_official_quota_exhaustion(&json_payload(status, value)));
        }
    }
    assert!(!is_official_quota_exhaustion(&json_payload(
        500,
        json!({ "error": { "code": "insufficient_quota" } })
    )));
}

#[test]
fn reliability_transient_and_unknown_429_retry_without_account_mutation() {
    for value in [
        json!({ "error": { "code": "rate_limit_exceeded" } }),
        json!({ "error": { "message": "limit" } }),
        Value::Null,
    ] {
        let mut requests = 0;
        let mut elapsed = Duration::ZERO;
        let mut waits = Vec::new();
        let payload = retry_upstream_request_with(
            Duration::from_secs(4),
            || {
                requests += 1;
                let mut response = official_payload(429, 0);
                response.body = UpstreamBody::Buffered(serde_json::to_vec(&value).unwrap());
                Ok(response)
            },
            |_, _| panic!("Non-quota 429 must not mutate account state"),
            |delay| {
                waits.push(delay.as_secs());
                elapsed += delay;
                elapsed
            },
        )
        .unwrap();
        assert_eq!(requests, 2);
        assert_eq!(waits, [1, 3]);
        assert_eq!(payload.status, 429);
        let state = ManagerStateFile {
            auto_switch_on_quota_exhaustion: true,
            auto_disable_unreachable_accounts: true,
            ..Default::default()
        };
        assert!(
            account_to_disable_after_429_timeout(&payload, &state, &AppSettings::default())
                .is_none()
        );
    }
}

#[test]
fn reliability_retry_after_is_forwarded_and_honored_within_deadline() {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("retry-after", "3600".parse().unwrap());
    headers.insert("set-cookie", "private-cookie".parse().unwrap());
    assert_eq!(
        forwarded_response_headers(&headers),
        vec![("retry-after".to_string(), "3600".to_string())]
    );
    let mut requests = 0;
    let mut waits = Vec::new();
    let response = retry_upstream_request_with(
        Duration::from_secs(3),
        || {
            requests += 1;
            let mut payload =
                json_payload(429, json!({ "error": { "code": "rate_limit_exceeded" } }));
            payload.response_headers = forwarded_response_headers(&headers);
            Ok(payload)
        },
        |_, _| panic!("Rate limits are not exhausted accounts"),
        |delay| {
            waits.push(delay);
            delay
        },
    )
    .unwrap();
    assert_eq!(requests, 1);
    assert_eq!(waits, [Duration::from_secs(3)]);
    assert_eq!(response.response_headers[0].1, "3600");
}

#[test]
fn reliability_retry_after_supports_dates_and_rejects_invalid_values() {
    let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    for (value, expected) in [
        ("7", 7),
        ("Tue, 14 Nov 2023 22:13:27 GMT", 7),
        ("0", 1),
        ("Tue, 14 Nov 2023 22:13:19 GMT", 1),
        ("-2", 3),
        ("invalid", 3),
    ] {
        let mut payload = json_payload(429, json!({}));
        payload.response_headers = vec![("Retry-After".to_string(), value.to_string())];
        assert_eq!(
            upstream_retry_delay(&payload, 2, now),
            Duration::from_secs(expected),
            "{value}"
        );
    }
}

#[test]
fn reliability_extreme_retry_after_is_clamped_before_waiting() {
    for header in [
        u64::MAX.to_string(),
        "Fri, 31 Dec 9999 23:59:59 GMT".to_string(),
    ] {
        let mut requests = 0;
        let response = retry_upstream_request_with(
            Duration::from_secs(2),
            || {
                requests += 1;
                let mut payload =
                    json_payload(429, json!({ "error": { "message": "unclassified limit" } }));
                payload.response_headers = vec![("retry-after".to_string(), header.clone())];
                Ok(payload)
            },
            |_, _| panic!("Unknown 429 must not mutate account state"),
            |delay| {
                assert_eq!(delay, Duration::from_secs(2));
                delay
            },
        )
        .unwrap();
        assert_eq!(requests, 1);
        assert_eq!(response.response_headers[0].1, header);
    }
}

#[test]
fn reliability_local_upstream_rate_limit_recovers_same_model_without_quota_switch() {
    let server = Server::http("127.0.0.1:0").unwrap();
    let address = server.server_addr().to_ip().unwrap();
    let (sender, receiver) = mpsc::channel();
    let handle = thread::spawn(move || {
        for attempt in 0..2 {
            let mut request = server
                .recv_timeout(Duration::from_secs(3))
                .unwrap()
                .expect("local request");
            let mut body = String::new();
            request.as_reader().read_to_string(&mut body).unwrap();
            sender
                .send((
                    request.url().to_string(),
                    serde_json::from_str::<Value>(&body).unwrap(),
                ))
                .unwrap();
            if attempt == 0 {
                request
                    .respond(
                        Response::from_string(
                            json!({ "error": { "type": "rate_limit_error" } }).to_string(),
                        )
                        .with_status_code(429)
                        .with_header(Header::from_bytes("Retry-After", "2").unwrap())
                        .with_header(
                            Header::from_bytes("Content-Type", "application/json").unwrap(),
                        ),
                    )
                    .unwrap();
            } else {
                request.respond(Response::from_string(concat!(
                    "data: {\"choices\":[{\"delta\":{\"content\":\"Recovered\"},\"finish_reason\":\"stop\"}]}\n\n",
                    "data: [DONE]\n\n"
                )).with_header(Header::from_bytes("Content-Type", "text/event-stream").unwrap())).unwrap();
            }
        }
    });
    let mut provider = api_detection_provider(format!("http://{address}"));
    provider.id = uuid::Uuid::new_v4().to_string();
    provider.model_selection_controlled_by_codex = true;
    provider
        .model_api_formats
        .insert("model-b".to_string(), ProviderApiFormat::OpenaiChat);
    let body = serde_json::to_vec(&json!({ "model": "model-b", "input": "hello", "stream": true }))
        .unwrap();
    let mut waits = Vec::new();
    let result = retry_upstream_request_with(
        Duration::from_secs(5),
        || forward_provider_request(&Method::Post, "/v1/responses", &[], body.clone(), &provider),
        |_, _| panic!("Rate limit must not switch/disable an account"),
        |delay| {
            waits.push(delay);
            delay
        },
    )
    .unwrap();
    assert_eq!(result.status, 200);
    let output = String::from_utf8(read_upstream_payload(result)).unwrap();
    assert_reliability_terminal(&output, "response.completed");
    assert_eq!(waits, [Duration::from_secs(2)]);
    handle.join().unwrap();
    for _ in 0..2 {
        let (path, request) = receiver.recv().unwrap();
        assert_eq!(path, "/v1/chat/completions");
        assert_eq!(request["model"], "model-b");
    }
}

#[test]
fn reliability_client_selected_unknown_model_is_rejected_before_network() {
    let mut provider = api_detection_provider("http://127.0.0.1:1".to_string());
    provider.model_selection_controlled_by_codex = true;
    let body = api_detection_body("unconfigured-model");
    let selected: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        selected_provider_model(&selected, &provider),
        "unconfigured-model"
    );
    for response in [
        forward_provider_request(&Method::Post, "/v1/responses", &[], body.clone(), &provider),
        forward_chat_bridge(&Method::Post, "/v1/responses", &[], body.clone(), &provider),
        forward_provider(
            &Method::Post,
            "/v1/responses/compact",
            &[],
            body.clone(),
            &provider,
        ),
    ] {
        let payload = response.unwrap();
        assert_eq!(payload.status, 400);
        let value: Value = serde_json::from_slice(&read_upstream_payload(payload)).unwrap();
        assert_eq!(value["error"]["code"], "model_not_configured");
        assert_eq!(value["error"]["param"], "model");
    }
    for body in [
        json!({ "model": "model-b" }),
        json!({}),
        json!({ "model": "  " }),
    ] {
        assert!(provider_model_error(&body, &provider).is_none());
    }
}

#[test]
fn reliability_explicit_fixed_mode_and_official_dynamic_models_remain_supported() {
    let mut provider = api_detection_provider("http://127.0.0.1:1".to_string());
    provider.model_selection_controlled_by_codex = false;
    let body = json!({ "model": "client-placeholder" });
    assert!(provider_model_error(&body, &provider).is_none());
    assert_eq!(selected_provider_model(&body, &provider), "model-a");
    let rewritten: Value = serde_json::from_slice(&provider_body_for_upstream(
        &Method::Post,
        "/v1/responses",
        serde_json::to_vec(&body).unwrap(),
        &provider,
    ))
    .unwrap();
    assert_eq!(rewritten["model"], "model-a");
    let provider = openai_provider("http://127.0.0.1:1".to_string());
    assert!(provider_model_error(&body, &provider).is_none());
    assert_eq!(
        selected_provider_model(&body, &provider),
        "client-placeholder"
    );
}
