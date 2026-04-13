use std::env;
use std::time::Duration;

use serde_json::{json, Value};

const SLACK_BASE: &str = "https://slack.com/api";

fn col_name() -> String {
    env::var("SLACK_COL_NAME").unwrap_or_else(|_| "Col0".to_string())
}

fn col_done() -> String {
    env::var("SLACK_COL_DONE").unwrap_or_else(|_| "Col00".to_string())
}

fn col_date() -> String {
    env::var("SLACK_COL_DATE").unwrap_or_else(|_| "Col02".to_string())
}

#[derive(Debug, Clone)]
pub struct ListItem {
    pub row_id: String,
    pub parent_id: Option<String>,
    pub name: Option<String>,
    pub done: bool,
}

fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| format!("erro ao construir client: {}", e))
}

fn check_ok(body: &Value) -> Result<(), String> {
    if body.get("ok").and_then(|v| v.as_bool()) == Some(true) {
        return Ok(());
    }
    let err = body
        .get("error")
        .and_then(|v| v.as_str())
        .unwrap_or("erro desconhecido");
    let needed = body.get("needed").and_then(|v| v.as_str()).unwrap_or("");
    if needed.is_empty() {
        Err(format!("Slack API erro: {}", err))
    } else {
        Err(format!("Slack API erro: {} (needed={})", err, needed))
    }
}

fn extract_field<'a>(item: &'a Value, key: &str) -> Option<&'a Value> {
    item.get("fields")?
        .as_array()?
        .iter()
        .find(|f| f.get("key").and_then(|v| v.as_str()) == Some(key))
}

pub async fn fetch_items(token: &str, list_id: &str) -> Result<Vec<ListItem>, String> {
    let client = http_client()?;
    let body: Value = client
        .post(format!("{}/slackLists.items.list", SLACK_BASE))
        .bearer_auth(token)
        .form(&[("list_id", list_id)])
        .send()
        .await
        .map_err(|e| format!("request slackLists.items.list falhou: {}", e))?
        .json()
        .await
        .map_err(|e| format!("json inválido: {}", e))?;
    check_ok(&body)?;

    let items = body
        .get("items")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "resposta sem campo `items`".to_string())?;

    let parsed = items
        .iter()
        .map(|it| {
            let row_id = it
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let parent_id = it
                .get("parent_record_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let name = extract_field(it, "name")
                .and_then(|f| f.get("text"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let done = extract_field(it, "todo_completed")
                .and_then(|f| f.get("checkbox"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            ListItem {
                row_id,
                parent_id,
                name,
                done,
            }
        })
        .collect();
    Ok(parsed)
}

fn rich_text_block(text: &str) -> Value {
    json!([{
        "type": "rich_text",
        "elements": [{
            "type": "rich_text_section",
            "elements": [{"type": "text", "text": text}]
        }]
    }])
}

pub async fn create_subtask(
    token: &str,
    list_id: &str,
    parent_row_id: &str,
    texto: &str,
    done: bool,
    date_iso: Option<&str>,
) -> Result<String, String> {
    let client = http_client()?;
    let mut initial_fields = vec![json!({
        "column_id": col_name(),
        "rich_text": rich_text_block(texto)
    })];
    if done {
        initial_fields.push(json!({
            "column_id": col_done(),
            "checkbox": true
        }));
    }
    if let Some(date) = date_iso {
        initial_fields.push(json!({
            "column_id": col_date(),
            "date": [date]
        }));
    }
    let payload = json!({
        "list_id": list_id,
        "parent_item_id": parent_row_id,
        "initial_fields": initial_fields
    });

    let body: Value = client
        .post(format!("{}/slackLists.items.create", SLACK_BASE))
        .bearer_auth(token)
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("request slackLists.items.create falhou: {}", e))?
        .json()
        .await
        .map_err(|e| format!("json inválido: {}", e))?;
    check_ok(&body)?;

    let row_id = body
        .pointer("/item/id")
        .or_else(|| body.pointer("/id"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Ok(row_id)
}

#[allow(dead_code)]
pub async fn delete_item(token: &str, list_id: &str, row_id: &str) -> Result<(), String> {
    let client = http_client()?;
    let payload = json!({
        "list_id": list_id,
        "id": row_id
    });

    let body: Value = client
        .post(format!("{}/slackLists.items.delete", SLACK_BASE))
        .bearer_auth(token)
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("request slackLists.items.delete falhou: {}", e))?
        .json()
        .await
        .map_err(|e| format!("json inválido: {}", e))?;
    check_ok(&body)
}

pub async fn mark_done(token: &str, list_id: &str, row_id: &str) -> Result<(), String> {
    let client = http_client()?;
    let payload = json!({
        "list_id": list_id,
        "cells": [{
            "row_id": row_id,
            "column_id": col_done(),
            "checkbox": true
        }]
    });

    let body: Value = client
        .post(format!("{}/slackLists.items.update", SLACK_BASE))
        .bearer_auth(token)
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("request slackLists.items.update falhou: {}", e))?
        .json()
        .await
        .map_err(|e| format!("json inválido: {}", e))?;
    check_ok(&body)
}
