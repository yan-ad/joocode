use std::collections::BTreeMap;

use serde_json::{Value, json};
use uuid::Uuid;

use crate::error::ApiError;

pub fn response_id() -> String {
    format!("resp_{}", Uuid::new_v4().simple())
}

pub fn anthropic_to_chat_request(request: &Value, upstream_model: &str) -> Result<Value, ApiError> {
    let mut messages = Vec::new();
    if let Some(system) = request.get("system") {
        let text = match system {
            Value::String(text) => text.clone(),
            Value::Array(parts) => parts
                .iter()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        };
        if !text.is_empty() {
            messages.push(json!({"role":"system","content":text}));
        }
    }
    for message in request
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user");
        let content = message.get("content").cloned().unwrap_or(Value::Null);
        if let Some(text) = content.as_str() {
            messages.push(json!({"role":role,"content":text}));
            continue;
        }
        let mut parts = Vec::new();
        for part in content.as_array().into_iter().flatten() {
            match part.get("type").and_then(Value::as_str) {
                Some("text") => parts.push(json!({
                    "type":"text",
                    "text":part.get("text").and_then(Value::as_str).unwrap_or_default()
                })),
                Some("image") => {
                    let source = part.get("source").unwrap_or(&Value::Null);
                    let url = if source.get("type").and_then(Value::as_str) == Some("base64") {
                        format!(
                            "data:{};base64,{}",
                            source.get("media_type").and_then(Value::as_str).unwrap_or("image/png"),
                            source.get("data").and_then(Value::as_str).unwrap_or_default()
                        )
                    } else {
                        source.get("url").and_then(Value::as_str).unwrap_or_default().to_owned()
                    };
                    parts.push(json!({"type":"image_url","image_url":{"url":url}}));
                }
                Some("tool_use") => messages.push(json!({
                    "role":"assistant",
                    "tool_calls":[{"id":part.get("id"),"type":"function","function":{
                        "name":part.get("name"),
                        "arguments":serde_json::to_string(part.get("input").unwrap_or(&json!({}))).unwrap_or_else(|_| "{}".into())
                    }}]
                })),
                Some("tool_result") => messages.push(json!({
                    "role":"tool",
                    "tool_call_id":part.get("tool_use_id"),
                    "content":content_to_text(part.get("content").unwrap_or(&Value::Null))
                })),
                _ => {}
            }
        }
        if !parts.is_empty() {
            messages.push(json!({"role":role,"content":parts}));
        }
    }
    let mut output = json!({
        "model": upstream_model,
        "messages": messages,
        "stream": request.get("stream").and_then(Value::as_bool).unwrap_or(false),
    });
    let object = output.as_object_mut().expect("object");
    copy_field(request, object, "max_tokens", "max_tokens");
    copy_field(request, object, "temperature", "temperature");
    copy_field(request, object, "top_p", "top_p");
    copy_field(request, object, "stop_sequences", "stop");
    if let Some(tools) = request.get("tools").and_then(Value::as_array) {
        object.insert(
            "tools".into(),
            Value::Array(
                tools
                    .iter()
                    .map(|tool| json!({"type":"function","function":{
                        "name":tool.get("name"),
                        "description":tool.get("description"),
                        "parameters":tool.get("input_schema").cloned().unwrap_or_else(|| json!({"type":"object"}))
                    }}))
                    .collect(),
            ),
        );
    }
    Ok(output)
}

pub fn chat_to_anthropic_response(chat: Value, requested_model: &str) -> Result<Value, ApiError> {
    let message = chat.pointer("/choices/0/message").ok_or_else(|| {
        ApiError::upstream(
            http::StatusCode::BAD_GATEWAY,
            "upstream response has no message",
        )
    })?;
    let mut content = Vec::new();
    if let Some(text) = message
        .get("content")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
    {
        content.push(json!({"type":"text","text":text}));
    }
    for call in message
        .get("tool_calls")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let input = call
            .pointer("/function/arguments")
            .and_then(Value::as_str)
            .and_then(|value| serde_json::from_str(value).ok())
            .unwrap_or_else(|| json!({}));
        content.push(json!({
            "type":"tool_use",
            "id":call.get("id").cloned().unwrap_or_else(|| Value::String(call_id())),
            "name":call.pointer("/function/name").cloned().unwrap_or_else(|| Value::String("tool".into())),
            "input":input
        }));
    }
    let usage = chat.get("usage").cloned().unwrap_or_else(|| json!({}));
    Ok(json!({
        "id":format!("msg_{}", Uuid::new_v4().simple()),
        "type":"message",
        "role":"assistant",
        "model":requested_model,
        "content":content,
        "stop_reason":if message.get("tool_calls").is_some() {"tool_use"} else {"end_turn"},
        "stop_sequence":null,
        "usage":{
            "input_tokens":usage.get("prompt_tokens").cloned().unwrap_or_else(|| json!(0)),
            "output_tokens":usage.get("completion_tokens").cloned().unwrap_or_else(|| json!(0))
        }
    }))
}
pub fn message_id() -> String {
    format!("msg_{}", Uuid::new_v4().simple())
}
pub fn call_id() -> String {
    format!("call_{}", Uuid::new_v4().simple())
}

pub fn to_chat_request(request: &Value, upstream_model: &str) -> Result<Value, ApiError> {
    let mut messages = Vec::new();
    if let Some(instructions) = request.get("instructions").and_then(Value::as_str) {
        messages.push(json!({ "role": "system", "content": instructions }));
    }
    convert_input(request.get("input"), &mut messages)?;

    let mut output = json!({
        "model": upstream_model,
        "messages": messages,
        "stream": request.get("stream").and_then(Value::as_bool).unwrap_or(false),
    });
    let object = output.as_object_mut().expect("object");
    copy_field(request, object, "temperature", "temperature");
    copy_field(request, object, "top_p", "top_p");
    copy_field(request, object, "max_output_tokens", "max_tokens");
    copy_field(
        request,
        object,
        "parallel_tool_calls",
        "parallel_tool_calls",
    );
    copy_field(request, object, "user", "user");
    if let Some(tools) = request.get("tools").and_then(Value::as_array) {
        object.insert(
            "tools".into(),
            Value::Array(tools.iter().filter_map(convert_tool).collect()),
        );
    }
    if let Some(choice) = request.get("tool_choice") {
        object.insert("tool_choice".into(), convert_tool_choice(choice));
    }
    if let Some(reasoning) = request.get("reasoning")
        && let Some(effort) = reasoning.get("effort")
    {
        object.insert("reasoning_effort".into(), effort.clone());
    }
    Ok(output)
}

fn copy_field(source: &Value, target: &mut serde_json::Map<String, Value>, from: &str, to: &str) {
    if let Some(value) = source.get(from) {
        target.insert(to.to_owned(), value.clone());
    }
}

fn convert_input(input: Option<&Value>, messages: &mut Vec<Value>) -> Result<(), ApiError> {
    match input {
        None => Err(ApiError::bad_request("missing 'input'")),
        Some(Value::String(text)) => {
            messages.push(json!({ "role": "user", "content": text }));
            Ok(())
        }
        Some(Value::Array(items)) => {
            for item in items {
                match item.get("type").and_then(Value::as_str).unwrap_or("message") {
                    "message" => messages.push(convert_message(item)?),
                    "function_call" => messages.push(json!({
                        "role": "assistant",
                        "tool_calls": [{
                            "id": item.get("call_id").or_else(|| item.get("id")).cloned().unwrap_or_else(|| Value::String(call_id())),
                            "type": "function",
                            "function": {
                                "name": item.get("name").cloned().unwrap_or(Value::String("tool".into())),
                                "arguments": item.get("arguments").cloned().unwrap_or(Value::String("{}".into()))
                            }
                        }]
                    })),
                    "function_call_output" => messages.push(json!({
                        "role": "tool",
                        "tool_call_id": item.get("call_id").cloned().unwrap_or(Value::Null),
                        "content": content_to_text(item.get("output").unwrap_or(&Value::Null))
                    })),
                    _ => {}
                }
            }
            Ok(())
        }
        _ => Err(ApiError::bad_request("'input' must be a string or array")),
    }
}

fn convert_message(item: &Value) -> Result<Value, ApiError> {
    let role = item.get("role").and_then(Value::as_str).unwrap_or("user");
    let content = item
        .get("content")
        .ok_or_else(|| ApiError::bad_request("message input is missing content"))?;
    let mut parts = Vec::new();
    match content {
        Value::String(text) => return Ok(json!({ "role": role, "content": text })),
        Value::Array(values) => {
            for part in values {
                match part.get("type").and_then(Value::as_str) {
                Some("input_text" | "output_text" | "text") => parts.push(json!({
                    "type": "text", "text": part.get("text").and_then(Value::as_str).unwrap_or_default()
                })),
                Some("input_image" | "image_url") => {
                    let url = part.get("image_url").or_else(|| part.get("url")).cloned().unwrap_or(Value::Null);
                    parts.push(json!({ "type": "image_url", "image_url": { "url": url } }));
                }
                _ => {}
            }
            }
        }
        _ => {}
    }
    Ok(json!({ "role": role, "content": parts }))
}

fn content_to_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

fn convert_tool(tool: &Value) -> Option<Value> {
    if tool.get("type").and_then(Value::as_str) != Some("function") {
        return None;
    }
    if let Some(function) = tool.get("function") {
        return Some(json!({ "type": "function", "function": function }));
    }
    Some(json!({ "type": "function", "function": {
        "name": tool.get("name")?,
        "description": tool.get("description").cloned().unwrap_or(Value::Null),
        "parameters": tool.get("parameters").cloned().unwrap_or_else(|| json!({"type":"object","properties":{}})),
        "strict": tool.get("strict").cloned().unwrap_or(Value::Bool(false))
    }}))
}

fn convert_tool_choice(choice: &Value) -> Value {
    if let Some(name) = choice.get("name") {
        json!({ "type": "function", "function": { "name": name } })
    } else {
        choice.clone()
    }
}

pub fn from_chat_response(
    chat: Value,
    requested_model: &str,
    id: String,
) -> Result<Value, ApiError> {
    let choice = chat
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|v| v.first())
        .ok_or_else(|| {
            ApiError::upstream(
                http::StatusCode::BAD_GATEWAY,
                "upstream response has no choices",
            )
        })?;
    let message = choice.get("message").cloned().unwrap_or(Value::Null);
    let mut output = Vec::new();
    if let Some(text) = message
        .get("content")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
    {
        output.push(json!({
            "id": message_id(), "type": "message", "status": "completed", "role": "assistant",
            "content": [{ "type": "output_text", "text": text, "annotations": [] }]
        }));
    }
    if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
        for call in calls {
            output.push(json!({
                "id": format!("fc_{}", Uuid::new_v4().simple()), "type": "function_call", "status": "completed",
                "call_id": call.get("id").cloned().unwrap_or_else(|| Value::String(call_id())),
                "name": call.pointer("/function/name").cloned().unwrap_or(Value::String("tool".into())),
                "arguments": call.pointer("/function/arguments").cloned().unwrap_or(Value::String("{}".into()))
            }));
        }
    }
    let usage = chat.get("usage").cloned().unwrap_or_else(|| json!({}));
    Ok(json!({
        "id": id, "object": "response", "created_at": now(), "status": "completed", "error": null,
        "model": requested_model, "output": output, "parallel_tool_calls": true,
        "usage": {
            "input_tokens": usage.get("prompt_tokens").cloned().unwrap_or(Value::Number(0.into())),
            "output_tokens": usage.get("completion_tokens").cloned().unwrap_or(Value::Number(0.into())),
            "total_tokens": usage.get("total_tokens").cloned().unwrap_or(Value::Number(0.into()))
        }
    }))
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Default)]
pub struct StreamState {
    pub response_id: String,
    pub requested_model: String,
    pub message_id: String,
    pub text: String,
    pub calls: BTreeMap<usize, ToolCallState>,
    pub usage: Value,
}

#[derive(Default)]
pub struct ToolCallState {
    pub item_id: String,
    pub call_id: String,
    pub name: String,
    pub arguments: String,
    pub announced: bool,
}

impl StreamState {
    pub fn new(response_id: String, requested_model: String) -> Self {
        Self {
            response_id,
            requested_model,
            message_id: message_id(),
            ..Default::default()
        }
    }

    pub fn created_events(&self) -> Vec<String> {
        vec![
            event(
                "response.created",
                json!({ "response": self.response("in_progress", vec![]) }),
            ),
            event(
                "response.in_progress",
                json!({ "response": self.response("in_progress", vec![]) }),
            ),
            event(
                "response.output_item.added",
                json!({ "output_index": 0, "item": {
                    "id": self.message_id, "type": "message", "status": "in_progress", "role": "assistant", "content": []
                }}),
            ),
            event(
                "response.content_part.added",
                json!({ "item_id": self.message_id, "output_index": 0, "content_index": 0,
                    "part": { "type": "output_text", "text": "", "annotations": [] }
                }),
            ),
        ]
    }

    pub fn consume_chunk(&mut self, chunk: &Value) -> Vec<String> {
        if let Some(usage) = chunk.get("usage") {
            self.usage = usage.clone();
        }
        let Some(delta) = chunk.pointer("/choices/0/delta") else {
            return vec![];
        };
        let mut events = Vec::new();
        if let Some(text) = delta.get("content").and_then(Value::as_str) {
            self.text.push_str(text);
            events.push(event(
                "response.output_text.delta",
                json!({
                    "item_id": self.message_id, "output_index": 0, "content_index": 0, "delta": text
                }),
            ));
        }
        if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for call in calls {
                let index = call.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                let state = self.calls.entry(index).or_insert_with(|| ToolCallState {
                    item_id: format!("fc_{}", Uuid::new_v4().simple()),
                    ..Default::default()
                });
                if let Some(id) = call.get("id").and_then(Value::as_str) {
                    state.call_id.push_str(id);
                }
                if let Some(name) = call.pointer("/function/name").and_then(Value::as_str) {
                    state.name.push_str(name);
                }
                if !state.announced && (!state.call_id.is_empty() || !state.name.is_empty()) {
                    if state.call_id.is_empty() {
                        state.call_id = call_id();
                    }
                    events.push(event(
                        "response.output_item.added",
                        json!({ "output_index": index + 1, "item": {
                            "id": state.item_id, "type": "function_call", "status": "in_progress",
                            "call_id": state.call_id, "name": state.name, "arguments": ""
                        }}),
                    ));
                    state.announced = true;
                }
                if let Some(arguments) = call.pointer("/function/arguments").and_then(Value::as_str)
                {
                    state.arguments.push_str(arguments);
                    events.push(event(
                        "response.function_call_arguments.delta",
                        json!({
                            "item_id": state.item_id, "output_index": index + 1, "delta": arguments
                        }),
                    ));
                }
            }
        }
        events
    }

    pub fn completed_events(&self) -> Vec<String> {
        let mut events = vec![
            event(
                "response.output_text.done",
                json!({ "item_id": self.message_id, "output_index": 0, "content_index": 0, "text": self.text }),
            ),
            event(
                "response.content_part.done",
                json!({ "item_id": self.message_id, "output_index": 0, "content_index": 0,
                    "part": { "type": "output_text", "text": self.text, "annotations": [] }
                }),
            ),
            event(
                "response.output_item.done",
                json!({ "output_index": 0, "item": {
                    "id": self.message_id, "type": "message", "status": "completed", "role": "assistant",
                    "content": [{ "type": "output_text", "text": self.text, "annotations": [] }]
                }}),
            ),
        ];
        for (index, call) in &self.calls {
            events.push(event(
                "response.function_call_arguments.done",
                json!({
                    "item_id": call.item_id, "output_index": index + 1, "arguments": call.arguments
                }),
            ));
            events.push(event("response.output_item.done", json!({ "output_index": index + 1, "item": {
                "id": call.item_id, "type": "function_call", "status": "completed", "call_id": call.call_id,
                "name": call.name, "arguments": call.arguments
            }})));
        }
        events.push(event(
            "response.completed",
            json!({ "response": self.response("completed", self.output()) }),
        ));
        events.push("data: [DONE]\n\n".into());
        events
    }

    fn output(&self) -> Vec<Value> {
        let mut output = vec![
            json!({ "id": self.message_id, "type": "message", "status": "completed", "role": "assistant",
            "content": [{ "type": "output_text", "text": self.text, "annotations": [] }] }),
        ];
        for call in self.calls.values() {
            output.push(json!({ "id": call.item_id, "type": "function_call", "status": "completed", "call_id": call.call_id,
                "name": call.name, "arguments": call.arguments }));
        }
        output
    }

    fn response(&self, status: &str, output: Vec<Value>) -> Value {
        let prompt = self
            .usage
            .get("prompt_tokens")
            .cloned()
            .unwrap_or_else(|| json!(0));
        let completion = self
            .usage
            .get("completion_tokens")
            .cloned()
            .unwrap_or_else(|| json!(0));
        let total = self
            .usage
            .get("total_tokens")
            .cloned()
            .unwrap_or_else(|| json!(0));
        json!({ "id": self.response_id, "object": "response", "created_at": now(), "status": status, "error": null,
            "model": self.requested_model, "output": output,
            "usage": { "input_tokens": prompt, "output_tokens": completion, "total_tokens": total }
        })
    }
}

pub fn event(name: &str, data: Value) -> String {
    let mut payload = match data {
        Value::Object(object) => object,
        value => serde_json::Map::from_iter([("data".into(), value)]),
    };
    payload.insert("type".into(), Value::String(name.into()));
    payload.insert("sequence_number".into(), json!(0));
    format!(
        "event: {name}\ndata: {}\n\n",
        serde_json::to_string(&Value::Object(payload)).unwrap()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_responses_input_and_tools() {
        let request = json!({
            "model": "demo/code", "instructions": "Be terse", "input": [{
                "type": "message", "role": "user", "content": [{"type":"input_text","text":"hello"}]
            }],
            "tools": [{"type":"function","name":"read","description":"Read","parameters":{"type":"object"}}]
        });
        let chat = to_chat_request(&request, "code").unwrap();
        assert_eq!(chat["messages"][0]["role"], "system");
        assert_eq!(chat["messages"][1]["content"][0]["text"], "hello");
        assert_eq!(chat["tools"][0]["function"]["name"], "read");
    }

    #[test]
    fn converts_chat_response() {
        let result = from_chat_response(
            json!({
                "choices": [{"message":{"content":"done","tool_calls":[]}}],
                "usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}
            }),
            "demo/code",
            "resp_test".into(),
        )
        .unwrap();
        assert_eq!(result["output"][0]["content"][0]["text"], "done");
        assert_eq!(result["usage"]["total_tokens"], 3);
    }

    #[test]
    fn converts_anthropic_messages_and_response() {
        let request = json!({
            "model":"claude-joocode/demo/model-a",
            "max_tokens":100,
            "messages":[{"role":"user","content":[{"type":"text","text":"hello"}]}],
            "tools":[{"name":"read","input_schema":{"type":"object"}}]
        });
        let chat = anthropic_to_chat_request(&request, "model-a").unwrap();
        assert_eq!(chat["model"], "model-a");
        assert_eq!(chat["messages"][0]["content"][0]["text"], "hello");
        assert_eq!(chat["tools"][0]["function"]["name"], "read");

        let response = chat_to_anthropic_response(
            json!({"choices":[{"message":{"content":"done"}}],"usage":{"prompt_tokens":2,"completion_tokens":1}}),
            "claude-joocode/demo/model-a",
        )
        .unwrap();
        assert_eq!(response["content"][0]["text"], "done");
        assert_eq!(response["usage"]["input_tokens"], 2);
    }
}
