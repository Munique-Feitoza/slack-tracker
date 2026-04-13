use std::env;
use std::time::Duration;

use serde_json::json;

pub async fn send_to_slack(summary: &str) -> Result<(), String> {
    let webhook = env::var("SLACK_WEBHOOK_URL")
        .map_err(|_| "SLACK_WEBHOOK_URL não definido".to_string())?;

    let payload = json!({
        "text": format!("Resumo do dia:\n{}", summary),
    });

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| format!("erro ao construir client: {}", e))?;

    let resp = client
        .post(&webhook)
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("erro na request Slack: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("Slack {}: {}", status, text));
    }
    Ok(())
}
