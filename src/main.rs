mod analyzer;
mod lists;
mod slack;

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand};
use log::{error, info, warn};
use once_cell::sync::Lazy;
use regex::Regex;
use rusqlite::{params, Connection};

const POLL_INTERVAL_SECS: u64 = 60;

#[derive(Debug, Clone)]
struct ActivityLog {
    timestamp: DateTime<Utc>,
    nome_da_janela: String,
    nome_do_projeto: Option<String>,
}

static PROJECT_REGEXES: Lazy<Vec<(Regex, &'static str)>> = Lazy::new(|| {
    vec![
        (
            Regex::new(r"(?i)(.+?)\s*[-–—]\s*Visual Studio Code").unwrap(),
            "VSCode",
        ),
        (
            Regex::new(r"(?i)(.+?)\s*[-–—]\s*VSCode").unwrap(),
            "VSCode",
        ),
        (
            Regex::new(r"(?i)(.+?)\s*[-–—]\s*RustRover").unwrap(),
            "RustRover",
        ),
        (
            Regex::new(r"(?i)(.+?)\s*[-–—]\s*IntelliJ IDEA").unwrap(),
            "IntelliJ",
        ),
        (
            Regex::new(r"(?i)(.+?)\s*[-–—]\s*PyCharm").unwrap(),
            "PyCharm",
        ),
        (
            Regex::new(r"(?i)NVIM\s*[-–—]?\s*(.+)").unwrap(),
            "Nvim",
        ),
        (
            Regex::new(r"(?i)(.+?)\s*[-–—]\s*n?vim").unwrap(),
            "Nvim",
        ),
        (
            Regex::new(r"(?i)(.+?)\s*[-–—]\s*Sublime Text").unwrap(),
            "Sublime",
        ),
    ]
});

fn extract_project(window_name: &str) -> Option<String> {
    for (re, editor) in PROJECT_REGEXES.iter() {
        if let Some(caps) = re.captures(window_name) {
            if let Some(m) = caps.get(1) {
                let file_or_proj = m.as_str().trim().to_string();
                if !file_or_proj.is_empty() {
                    return Some(format!("{}:{}", editor, file_or_proj));
                }
                return Some((*editor).to_string());
            }
            return Some((*editor).to_string());
        }
    }
    None
}

fn get_active_window() -> Result<String, String> {
    if let Ok(out) = Command::new("xdotool")
        .args(["getactivewindow", "getwindowname"])
        .output()
    {
        if out.status.success() {
            let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !name.is_empty() {
                return Ok(name);
            }
        }
    }
    xprop_active_window()
}

fn xprop_active_window() -> Result<String, String> {
    let out = Command::new("xprop")
        .args(["-root", "_NET_ACTIVE_WINDOW"])
        .output()
        .map_err(|e| format!("falha ao executar xprop: {}", e))?;
    if !out.status.success() {
        return Err(format!(
            "xprop -root falhou: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let line = String::from_utf8_lossy(&out.stdout);
    let wid = line
        .split('#')
        .nth(1)
        .map(|s| s.trim().split_whitespace().next().unwrap_or("").to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("window id não encontrado em: {}", line.trim()))?;

    let out = Command::new("xprop")
        .args(["-id", &wid, "_NET_WM_NAME", "WM_NAME"])
        .output()
        .map_err(|e| format!("falha ao executar xprop -id: {}", e))?;
    if !out.status.success() {
        return Err(format!(
            "xprop -id falhou: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let txt = String::from_utf8_lossy(&out.stdout);
    for key in &["_NET_WM_NAME", "WM_NAME"] {
        for ln in txt.lines() {
            if ln.starts_with(key) {
                if let (Some(start), Some(end)) = (ln.find('"'), ln.rfind('"')) {
                    if end > start + 1 {
                        return Ok(ln[start + 1..end].to_string());
                    }
                }
            }
        }
    }
    Err("não achei nome da janela no output do xprop".to_string())
}

fn db_path() -> Result<PathBuf, String> {
    let base = dirs::data_local_dir()
        .ok_or_else(|| "não foi possível localizar ~/.local/share".to_string())?;
    let dir = base.join("slack-tracker");
    fs::create_dir_all(&dir).map_err(|e| format!("falha ao criar diretório de dados: {}", e))?;
    Ok(dir.join("logs.db"))
}

fn init_db(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS activity_log (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            timestamp TEXT NOT NULL,
            nome_da_janela TEXT NOT NULL,
            nome_do_projeto TEXT
        )",
        [],
    )?;
    Ok(())
}

fn insert_log(conn: &Connection, entry: &ActivityLog) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO activity_log (timestamp, nome_da_janela, nome_do_projeto)
         VALUES (?1, ?2, ?3)",
        params![
            entry.timestamp.to_rfc3339(),
            entry.nome_da_janela,
            entry.nome_do_projeto,
        ],
    )?;
    Ok(())
}

fn tick(conn: &Connection) {
    match get_active_window() {
        Ok(window) => {
            let entry = ActivityLog {
                timestamp: Utc::now(),
                nome_do_projeto: extract_project(&window),
                nome_da_janela: window,
            };
            match insert_log(conn, &entry) {
                Ok(_) => info!(
                    "logged: {} (projeto: {:?})",
                    entry.nome_da_janela, entry.nome_do_projeto
                ),
                Err(e) => error!("falha ao inserir no sqlite: {}", e),
            }
        }
        Err(e) => warn!("{}", e),
    }
}

#[derive(Parser, Debug)]
#[command(name = "slack-tracker", about = "Rastreador de atividade com resumo via LLM")]
struct Cli {
    #[command(subcommand)]
    command: CliCommand,
}

#[derive(Subcommand, Debug)]
enum CliCommand {
    /// Inicia o daemon de monitoramento da janela ativa.
    Start,
    /// Gera o resumo do dia e opcionalmente envia ao Slack.
    Report {
        /// Envia o resumo para o webhook do Slack (SLACK_WEBHOOK_URL).
        #[arg(long)]
        send: bool,
    },
    /// Gerencia a lista nativa do Slack (Slack Lists API).
    Todo {
        #[command(subcommand)]
        action: TodoAction,
    },
}

#[derive(Subcommand, Debug)]
enum TodoAction {
    /// Imprime a lista atual — hierarquia, row_ids, status — pra debug.
    Inspect,
    /// Coleta atividade de hoje, pede um plano à LLM e (com --apply) aplica na lista.
    Sync {
        /// Aplica as mudanças na Slack List. Sem essa flag, só imprime o que faria (dry-run).
        #[arg(long)]
        apply: bool,
    },
}

fn open_db() -> Result<(PathBuf, Connection), String> {
    let path = db_path()?;
    let conn = Connection::open(&path)
        .map_err(|e| format!("erro ao abrir sqlite em {:?}: {}", path, e))?;
    init_db(&conn).map_err(|e| format!("erro ao inicializar schema: {}", e))?;
    Ok((path, conn))
}

async fn run_start() -> Result<(), String> {
    let (path, conn) = open_db()?;
    info!(
        "slack-tracker iniciado. db={:?}, intervalo={}s",
        path, POLL_INTERVAL_SECS
    );

    tokio::spawn(async move {
        daily_sync_loop().await;
    });

    let mut interval = tokio::time::interval(Duration::from_secs(POLL_INTERVAL_SECS));
    loop {
        interval.tick().await;
        tick(&conn);
    }
}

fn last_sync_file() -> Result<PathBuf, String> {
    let base = dirs::data_local_dir()
        .ok_or_else(|| "sem data_local_dir".to_string())?;
    Ok(base.join("slack-tracker").join("last_sync_date"))
}

fn read_last_sync() -> Option<chrono::NaiveDate> {
    let p = last_sync_file().ok()?;
    let s = std::fs::read_to_string(&p).ok()?;
    chrono::NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok()
}

fn write_last_sync(date: chrono::NaiveDate) {
    if let Ok(p) = last_sync_file() {
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&p, date.format("%Y-%m-%d").to_string());
    }
}

fn parse_target_time() -> (u32, u32) {
    let raw = std::env::var("TODO_SYNC_TIME").unwrap_or_else(|_| "17:00".to_string());
    let mut parts = raw.split(':');
    let h: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(17);
    let m: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    (h.min(23), m.min(59))
}

fn apply_mode() -> bool {
    std::env::var("TODO_SYNC_APPLY")
        .map(|v| matches!(v.as_str(), "true" | "1" | "yes"))
        .unwrap_or(false)
}

fn next_fire_at(hour: u32, minute: u32) -> chrono::DateTime<chrono::Local> {
    let now = chrono::Local::now();
    let today = now.date_naive();
    let today_target = today
        .and_hms_opt(hour, minute, 0)
        .and_then(|dt| dt.and_local_timezone(chrono::Local).single())
        .unwrap_or(now);
    if today_target > now {
        today_target
    } else {
        let tomorrow = today + chrono::Duration::days(1);
        tomorrow
            .and_hms_opt(hour, minute, 0)
            .and_then(|dt| dt.and_local_timezone(chrono::Local).single())
            .unwrap_or(now)
    }
}

async fn daily_sync_loop() {
    let (hour, minute) = parse_target_time();
    let apply = apply_mode();
    info!(
        "scheduler iniciado — alvo diário {:02}:{:02} (apply={})",
        hour, minute, apply
    );

    let now = chrono::Local::now();
    let today = now.date_naive();
    let today_target_passed = now.time()
        >= chrono::NaiveTime::from_hms_opt(hour, minute, 0).unwrap_or_default();
    let already_synced_today = read_last_sync() == Some(today);
    if today_target_passed && !already_synced_today {
        info!("catch-up: não rodou hoje e já passou do horário alvo, disparando agora");
        if let Err(e) = run_todo_sync(apply).await {
            error!("catch-up sync falhou: {}", e);
        } else {
            write_last_sync(today);
        }
    }

    loop {
        let next = next_fire_at(hour, minute);
        let now = chrono::Local::now();
        let delay = (next - now).to_std().unwrap_or(Duration::from_secs(60));
        info!(
            "próximo todo sync: {} (em {}min)",
            next.format("%Y-%m-%d %H:%M:%S"),
            delay.as_secs() / 60
        );
        tokio::time::sleep(delay).await;
        let fire_date = chrono::Local::now().date_naive();
        if read_last_sync() == Some(fire_date) {
            info!("sync diário já rodou hoje, pulando");
            tokio::time::sleep(Duration::from_secs(120)).await;
            continue;
        }
        match run_todo_sync(apply).await {
            Ok(_) => {
                info!("sync diário executado com sucesso");
                write_last_sync(fire_date);
            }
            Err(e) => error!("sync diário falhou: {}", e),
        }
        tokio::time::sleep(Duration::from_secs(120)).await;
    }
}

fn slack_env() -> Result<(String, String), String> {
    let token = std::env::var("SLACK_API_TOKEN")
        .map_err(|_| "SLACK_API_TOKEN não definido".to_string())?;
    let list_id = std::env::var("SLACK_LIST_ID")
        .map_err(|_| "SLACK_LIST_ID não definido".to_string())?;
    Ok((token, list_id))
}

fn find_current_week_parent(items: &[lists::ListItem]) -> Option<&lists::ListItem> {
    let re = regex::Regex::new(r"(\d{2})/(\d{2})/(\d{4})\s*-\s*(\d{2})/(\d{2})/(\d{4})").ok()?;
    let today = chrono::Local::now().date_naive();
    for it in items.iter().filter(|i| i.parent_id.is_none()) {
        let name = match it.name.as_deref() {
            Some(n) => n,
            None => continue,
        };
        let Some(caps) = re.captures(name) else {
            continue;
        };
        let parse = |a: usize, b: usize, c: usize| -> Option<chrono::NaiveDate> {
            let d = caps.get(a)?.as_str().parse().ok()?;
            let m = caps.get(b)?.as_str().parse().ok()?;
            let y = caps.get(c)?.as_str().parse().ok()?;
            chrono::NaiveDate::from_ymd_opt(y, m, d)
        };
        let start = parse(1, 2, 3);
        let end = parse(4, 5, 6);
        if let (Some(s), Some(e)) = (start, end) {
            if today >= s && today <= e {
                return Some(it);
            }
        }
    }
    None
}

async fn run_todo_inspect() -> Result<(), String> {
    let (token, list_id) = slack_env()?;
    let items = lists::fetch_items(&token, &list_id).await?;
    println!("=== Lista {} ({} itens) ===\n", list_id, items.len());

    let parents: Vec<&lists::ListItem> =
        items.iter().filter(|i| i.parent_id.is_none()).collect();
    for p in &parents {
        let mark = if p.done { "[x]" } else { "[ ]" };
        let name = p.name.as_deref().unwrap_or("(sem nome)");
        println!("• {} {} (row_id={})", mark, name, p.row_id);
        for child in items.iter().filter(|i| i.parent_id.as_deref() == Some(&p.row_id)) {
            let cm = if child.done { "[x]" } else { "[ ]" };
            let cn = child.name.as_deref().unwrap_or("(sem nome)");
            println!("    ├─ {} {} (row_id={})", cm, cn, child.row_id);
        }
        println!();
    }

    if let Some(w) = find_current_week_parent(&items) {
        println!(
            "Semana atual detectada: {} (row_id={})",
            w.name.as_deref().unwrap_or("(sem nome)"),
            w.row_id
        );
    } else {
        println!("(nenhum pai da semana atual foi detectado pelo padrão DD/MM/YYYY - DD/MM/YYYY)");
    }
    Ok(())
}

async fn run_todo_sync(apply: bool) -> Result<(), String> {
    let (token, list_id) = slack_env()?;

    let items = lists::fetch_items(&token, &list_id).await?;
    let week = find_current_week_parent(&items)
        .ok_or_else(|| "nenhum pai da semana atual encontrado na lista".to_string())?;
    let week_row_id = week.row_id.clone();
    let week_name = week.name.clone().unwrap_or_default();
    let current_subtasks: Vec<lists::ListItem> = items
        .iter()
        .filter(|i| i.parent_id.as_deref() == Some(&week_row_id))
        .cloned()
        .collect();

    info!(
        "semana atual: {} ({}), {} subtarefas existentes",
        week_name,
        week_row_id,
        current_subtasks.len()
    );

    let snapshot = tokio::task::spawn_blocking(|| -> Result<analyzer::ActivitySnapshot, String> {
        let (_p, conn) = open_db()?;
        Ok(analyzer::collect_activity_today(&conn))
    })
    .await
    .map_err(|e| format!("spawn_blocking falhou: {}", e))??;

    let plan = analyzer::generate_sync_plan(&snapshot, &current_subtasks).await?;

    let subtasks_by_id: std::collections::HashMap<&str, &lists::ListItem> = current_subtasks
        .iter()
        .map(|i| (i.row_id.as_str(), i))
        .collect();

    println!(
        "\nSemana: {}\nModo: {}\n",
        week_name,
        if apply { "APLICAR" } else { "DRY RUN" }
    );

    println!("+ Novos itens ({}):", plan.novos.len());
    if plan.novos.is_empty() {
        println!("  (nenhum)");
    }
    for n in &plan.novos {
        println!("  + {}", n);
    }

    println!("\n✓ Marcar como feitos ({}):", plan.marcar_feito.len());
    if plan.marcar_feito.is_empty() {
        println!("  (nenhum)");
    }
    for r in &plan.marcar_feito {
        let nome = subtasks_by_id
            .get(r.as_str())
            .and_then(|i| i.name.as_deref())
            .unwrap_or("(sem nome)");
        println!("  ✓ {} ({})", nome, r);
    }

    if !apply {
        println!("\n(dry run — nada foi enviado ao Slack. Use --apply para efetivar.)");
        return Ok(());
    }

    println!("\nAplicando na Slack List...");
    let today_iso = chrono::Local::now().date_naive().format("%Y-%m-%d").to_string();
    for texto in &plan.novos {
        match lists::create_subtask(
            &token,
            &list_id,
            &week_row_id,
            texto,
            true,
            Some(&today_iso),
        )
        .await
        {
            Ok(id) => info!("criado (feito + data hoje): {} (row_id={})", texto, id),
            Err(e) => warn!("falha ao criar '{}': {}", texto, e),
        }
    }
    for r in &plan.marcar_feito {
        match lists::mark_done(&token, &list_id, r).await {
            Ok(_) => info!("marcado feito: {}", r),
            Err(e) => warn!("falha ao marcar {}: {}", r, e),
        }
    }
    println!("Pronto.");
    Ok(())
}

async fn run_report(send: bool) -> Result<(), String> {
    let summary = tokio::task::spawn_blocking(|| {
        let (_path, conn) = open_db()?;
        analyzer::generate_daily_summary(&conn)
    })
    .await
    .map_err(|e| format!("spawn_blocking falhou: {}", e))??;

    println!("{}", summary);

    if send {
        slack::send_to_slack(&summary).await?;
        info!("resumo enviado para o Slack");
    }
    Ok(())
}

#[tokio::main]
async fn main() {
    let _ = dotenvy::dotenv_override();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let cli = Cli::parse();
    let result = match cli.command {
        CliCommand::Start => run_start().await,
        CliCommand::Report { send } => run_report(send).await,
        CliCommand::Todo { action } => match action {
            TodoAction::Inspect => run_todo_inspect().await,
            TodoAction::Sync { apply } => run_todo_sync(apply).await,
        },
    };

    if let Err(e) = result {
        error!("{}", e);
        std::process::exit(1);
    }
}
