use serde_json::{json, Value};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use moonwakelens_core::{tools, Engine};

pub async fn run(engine: Arc<Engine>) -> anyhow::Result<()> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
    let writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(message) = rx.recv().await {
            let _ = stdout.write_all(format!("{message}\n").as_bytes()).await;
            let _ = stdout.flush().await;
        }
    });
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Some(line) = lines.next_line().await? {
        let Ok(request) = serde_json::from_str::<Value>(&line) else { continue };
        let Some(id) = request.get("id").cloned() else { continue };
        let (engine, tx) = (engine.clone(), tx.clone());
        tokio::spawn(async move {
            let _ = tx.send(respond(&engine, id, &request).await);
        });
    }
    drop(tx);
    writer.await?;
    Ok(())
}

async fn respond(engine: &Engine, id: Value, request: &Value) -> Value {
    let params = request.get("params").cloned().unwrap_or(Value::Null);
    let result = match request.get("method").and_then(Value::as_str).unwrap_or_default() {
        "initialize" => json!({
            "protocolVersion": params.get("protocolVersion").cloned().unwrap_or(json!("2025-06-18")),
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "moonwakelens", "version": env!("CARGO_PKG_VERSION")},
            "instructions": "YouTube research from public YouTube data. Start with `trending` for what is blowing up in a niche, `discover` for formats that beat their channel's normal views (presets: outliers, hidden_gems, underdogs, blowing_up, trending_shorts, todays_top, evergreens, recent_uploads), and `keyword_research` for video topics people search."
        }),
        "ping" => json!({}),
        "tools/list" => json!({"tools": tools::definitions()}),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or_default();
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            match tools::call(engine, name, &args).await {
                Ok(value) => json!({"content": [{"type": "text", "text": serde_json::to_string_pretty(&value).unwrap_or_default()}]}),
                Err(error) => json!({"content": [{"type": "text", "text": error.to_string()}], "isError": true}),
            }
        }
        method => return json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": format!("method not found: {method}")}}),
    };
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}
