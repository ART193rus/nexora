//! Потоковый ход диалога с движком (OpenAI-совместимый SSE) с поддержкой вызовов инструментов.
use serde_json::{json, Value};
use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub enum Ev {
    Text(String),
    Think(String),
}

#[derive(Clone, Debug)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub args: String,
}

pub struct Turn {
    pub content: String,
    pub calls: Vec<ToolCall>,
}

/// Один ход модели: текст (по мере поступления через `on`) и вызовы инструментов (по завершении).
pub fn stream_turn(
    base: &str,
    token: &str,
    alias: &str,
    messages: &[Value],
    tools: Option<&Value>,
    cancel: &AtomicBool,
    mut on: impl FnMut(Ev),
) -> Result<Turn, String> {
    let mut body = json!({
        "model": alias, "messages": messages, "stream": true,
        "temperature": 0.7, "top_p": 0.8, "top_k": 20, "repeat_penalty": 1.05
    });
    if let Some(t) = tools {
        body["tools"] = t.clone();
    }
    // переключатель «Рассуждение» выключен: модели с режимом размышления отвечают сразу (параметры шаблона; остальные модели их игнорируют)
    if !crate::prefs::think() {
        body["chat_template_kwargs"] = json!({"enable_thinking": false});
        body["reasoning_effort"] = json!("low");
    }
    let resp = ureq::post(&format!("{base}/v1/chat/completions"))
        .set("Authorization", &format!("Bearer {token}"))
        .timeout(Duration::from_secs(1800))
        .send_json(body)
        .map_err(|e| match e {
            ureq::Error::Status(c, r) => format!("движок ответил {c}: {}", r.into_string().unwrap_or_default()),
            other => other.to_string(),
        })?;
    let mut content = String::new();
    let mut calls: Vec<ToolCall> = vec![];
    for line in BufReader::new(resp.into_reader()).lines() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let line = line.map_err(|e| e.to_string())?;
        let Some(data) = line.strip_prefix("data: ") else { continue };
        if data == "[DONE]" {
            break;
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else { continue };
        let d = &v["choices"][0]["delta"];
        if let Some(t) = d["reasoning_content"].as_str().or_else(|| d["reasoning"].as_str()) {
            if !t.is_empty() {
                on(Ev::Think(t.to_string()));
            }
        }
        if let Some(t) = d["content"].as_str() {
            if !t.is_empty() {
                content.push_str(t);
                on(Ev::Text(t.to_string()));
            }
        }
        if let Some(arr) = d["tool_calls"].as_array() {
            for tc in arr {
                let idx = tc["index"].as_u64().unwrap_or(0) as usize;
                while calls.len() <= idx {
                    calls.push(ToolCall { id: String::new(), name: String::new(), args: String::new() });
                }
                let c = &mut calls[idx];
                if let Some(id) = tc["id"].as_str() {
                    if !id.is_empty() {
                        c.id = id.to_string();
                    }
                }
                if let Some(n) = tc["function"]["name"].as_str() {
                    c.name.push_str(n);
                }
                if let Some(a) = tc["function"]["arguments"].as_str() {
                    c.args.push_str(a);
                }
            }
        }
    }
    for (i, c) in calls.iter_mut().enumerate() {
        if c.id.is_empty() {
            c.id = format!("call_{i}");
        }
    }
    Ok(Turn { content, calls })
}
