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
        (
            Regex::new(r"(?i)(.+?)\s*[-–—]\s*Antigravity").unwrap(),
            "Antigravity",
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
    /// Consolida itens de uma semana que excedeu 50 subtarefas (limite Slack).
    /// Pede pra LLM reescrever a lista agrupando itens relacionados, sem perder informação.
    Consolidate {
        /// Data dentro da semana a consolidar (YYYY-MM-DD).
        #[arg(long = "week")]
        week: String,
        /// Aplica as mudanças (deleta antigos + cria consolidados). Sem essa flag, dry-run.
        #[arg(long)]
        apply: bool,
    },
    /// Adiciona itens prontos (um texto por linha lido do stdin) como subtarefas da semana.
    /// Útil quando os providers LLM estão indisponíveis e os nomes são gerados externamente.
    Add {
        /// Data dentro da semana onde inserir os itens (YYYY-MM-DD).
        #[arg(long = "week")]
        week: String,
        /// Data a setar em cada item (YYYY-MM-DD). Padrão: igual a --week.
        #[arg(long)]
        date: Option<String>,
        /// Marca cada item criado como feito.
        #[arg(long)]
        done: bool,
        /// Aplica de fato. Sem essa flag, só imprime o que faria (dry-run).
        #[arg(long)]
        apply: bool,
    },
    /// Coleta atividade de uma data (ou range) e aplica na lista.
    Sync {
        /// Aplica as mudanças na Slack List. Sem essa flag, só imprime o que faria (dry-run).
        #[arg(long)]
        apply: bool,
        /// Data alvo no formato YYYY-MM-DD (padrão: hoje). Conflita com --from/--to.
        #[arg(long)]
        date: Option<String>,
        /// Início do range (YYYY-MM-DD). Use junto com --to para sincronizar vários dias num único call do LLM.
        #[arg(long)]
        from: Option<String>,
        /// Fim do range (YYYY-MM-DD). Use junto com --from.
        #[arg(long)]
        to: Option<String>,
        /// Data dentro da semana onde os itens devem ser inseridos (padrão: igual à data/`to`).
        /// Use para retroativos: --from 2026-04-25 --to 2026-04-27 --target-week 2026-04-28
        /// coloca atividade de sáb-seg dentro do parent da semana atual.
        #[arg(long = "target-week")]
        target_week: Option<String>,
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

async fn daily_sync_loop() {
    let (hour, minute) = parse_target_time();
    let apply = apply_mode();
    info!(
        "scheduler iniciado — alvo diário {:02}:{:02} (apply={})",
        hour, minute, apply
    );

    // Poll-based: acorda a cada 60s e checa o relógio de parede.
    // Resistente a suspend/hibernate (CLOCK_MONOTONIC do tokio::sleep não
    // avança durante suspensão, então sleeps longos disparam tarde).
    let mut last_failure_at: Option<chrono::DateTime<chrono::Local>> = None;
    let cooldown = chrono::Duration::minutes(30);
    loop {
        let now = chrono::Local::now();
        let today = now.date_naive();
        let target = chrono::NaiveTime::from_hms_opt(hour, minute, 0).unwrap_or_default();
        let last_sync = read_last_sync();
        let already_synced_today = last_sync == Some(today);
        let target_passed_today = now.time() >= target;
        let in_cooldown = last_failure_at
            .map(|t| (now - t) < cooldown)
            .unwrap_or(false);

        if target_passed_today && !already_synced_today && !in_cooldown {
            // Backfill: se perdemos dias (daemon offline, suspend, etc.),
            // o range cobre desde last_sync+1 até hoje. Dividimos por semana
            // pra cada chunk ir pro parent certo (semanas começam na segunda).
            let from = match last_sync {
                Some(d) if d < today => d + chrono::Duration::days(1),
                _ => today,
            };
            let chunks = split_range_by_week(from, today);
            info!(
                "disparando sync diário: range {} → {} ({} chunk(s) de semana)",
                from,
                today,
                chunks.len()
            );
            let mut all_ok = true;
            for (chunk_from, chunk_to) in chunks {
                info!("  chunk: {} → {}", chunk_from, chunk_to);
                match run_todo_sync(apply, chunk_from, chunk_to, chunk_to).await {
                    Ok(_) => info!("  chunk {} → {} OK", chunk_from, chunk_to),
                    Err(e) => {
                        error!("  chunk {} → {} falhou: {}", chunk_from, chunk_to, e);
                        all_ok = false;
                        break;
                    }
                }
            }
            if all_ok {
                info!("sync diário executado com sucesso");
                write_last_sync(today);
                last_failure_at = None;
            } else {
                error!("sync diário falhou em algum chunk (próximo retry em 30min)");
                last_failure_at = Some(now);
            }
        }

        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}

fn slack_env() -> Result<(String, String), String> {
    let token = std::env::var("SLACK_API_TOKEN")
        .map_err(|_| "SLACK_API_TOKEN não definido".to_string())?;
    let list_id = std::env::var("SLACK_LIST_ID")
        .map_err(|_| "SLACK_LIST_ID não definido".to_string())?;
    Ok((token, list_id))
}

fn find_current_week_parent(items: &[lists::ListItem], date: chrono::NaiveDate) -> Option<&lists::ListItem> {
    let re = regex::Regex::new(r"(\d{2})/(\d{2})/(\d{4})\s*-\s*(\d{2})/(\d{2})/(\d{4})").ok()?;
    let today = date;
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

fn get_week_range(date: chrono::NaiveDate) -> (chrono::NaiveDate, chrono::NaiveDate) {
    use chrono::Datelike;
    let weekday = date.weekday();
    let days_from_monday = weekday.num_days_from_monday();
    let monday = date - chrono::Duration::days(days_from_monday as i64);
    let sunday = monday + chrono::Duration::days(6);
    (monday, sunday)
}

// Quebra um range [from..to] em chunks que cabem cada um numa única semana
// ISO (Mon-Sun). Necessário pra cada chunk ser sincronizado no parent certo.
fn split_range_by_week(
    from: chrono::NaiveDate,
    to: chrono::NaiveDate,
) -> Vec<(chrono::NaiveDate, chrono::NaiveDate)> {
    let mut chunks = Vec::new();
    let mut current = from;
    while current <= to {
        let (_mon, sun) = get_week_range(current);
        let chunk_to = sun.min(to);
        chunks.push((current, chunk_to));
        current = chunk_to + chrono::Duration::days(1);
    }
    chunks
}

async fn run_todo_inspect() -> Result<(), String> {
    let (token, list_id) = slack_env()?;

    println!("=== JSON completo do primeiro item (pra ver todos os campos) ===");
    match lists::fetch_first_item_raw(&token, &list_id).await {
        Ok(raw) => println!("{}", raw),
        Err(e) => println!("  (erro: {})", e),
    }
    println!();

    println!("=== Colunas disponíveis na lista ===");
    match lists::fetch_raw_fields(&token, &list_id).await {
        Ok(fields) => {
            for (key, tipo) in &fields {
                println!("  key={:20} type={}", key, tipo);
            }
        }
        Err(e) => println!("  (não foi possível ler colunas: {})", e),
    }
    println!();

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

    if let Some(w) = find_current_week_parent(&items, chrono::Local::now().date_naive()) {
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

async fn run_todo_consolidate(
    apply: bool,
    week_date: chrono::NaiveDate,
) -> Result<(), String> {
    let (token, list_id) = slack_env()?;
    let items = lists::fetch_items(&token, &list_id).await?;
    let parent = find_current_week_parent(&items, week_date).ok_or_else(|| {
        format!("nenhum parent encontrado para a semana de {}", week_date)
    })?;
    let week_row_id = parent.row_id.clone();
    let week_name = parent.name.clone().unwrap_or_default();

    let children: Vec<lists::ListItem> = items
        .iter()
        .filter(|i| i.parent_id.as_deref() == Some(&week_row_id))
        .cloned()
        .collect();

    info!(
        "semana {}: {} subtarefas existentes",
        week_name,
        children.len()
    );

    const MAX: usize = 50;
    if children.len() <= MAX {
        println!(
            "\nSemana {}: {} itens (≤{}), nada a consolidar.",
            week_name,
            children.len(),
            MAX
        );
        return Ok(());
    }

    let consolidated = analyzer::generate_consolidation_plan(&children, MAX).await?;

    println!(
        "\nSemana: {}\nModo: {}\n",
        week_name,
        if apply { "APLICAR" } else { "DRY RUN" }
    );
    println!(
        "Antes: {} itens | Depois: {} itens",
        children.len(),
        consolidated.len()
    );
    println!("\n--- Lista consolidada ---");
    for (i, t) in consolidated.iter().enumerate() {
        println!("  {:2}. {}", i + 1, t);
    }

    if !apply {
        println!("\n(dry run — nada foi enviado ao Slack. Use --apply para efetivar.)");
        return Ok(());
    }

    println!("\nDeletando {} itens antigos...", children.len());
    for c in &children {
        if let Err(e) = lists::delete_item(&token, &list_id, &c.row_id).await {
            warn!("falha ao deletar {}: {}", c.row_id, e);
        }
    }

    println!("Criando {} itens consolidados...", consolidated.len());
    let date_iso = week_date.format("%Y-%m-%d").to_string();
    for texto in &consolidated {
        match lists::create_subtask(
            &token,
            &list_id,
            &week_row_id,
            texto,
            true,
            Some(&date_iso),
        )
        .await
        {
            Ok(id) => info!("criado: {} ({})", texto, id),
            Err(e) => warn!("falha ao criar '{}': {}", texto, e),
        }
    }
    println!("Pronto.");
    Ok(())
}

async fn run_todo_add(
    week: chrono::NaiveDate,
    date: chrono::NaiveDate,
    done: bool,
    apply: bool,
) -> Result<(), String> {
    use std::io::BufRead;
    let (token, list_id) = slack_env()?;

    let raw_items: Vec<String> = std::io::stdin()
        .lock()
        .lines()
        .map_while(Result::ok)
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    if raw_items.is_empty() {
        return Err("nenhum item recebido no stdin (um texto por linha)".to_string());
    }
    let terms = analyzer::redact_terms();
    let items: Vec<String> = raw_items
        .into_iter()
        .filter(|t| {
            let dirty = analyzer::contains_redacted(t, &terms);
            if dirty {
                warn!("item descartado por conter termo de SLACK_REDACT_TERMS: {}", t);
            }
            !dirty
        })
        .collect();
    if items.is_empty() {
        return Err("todos os itens do stdin foram filtrados por SLACK_REDACT_TERMS".to_string());
    }

    let existing = lists::fetch_items(&token, &list_id).await?;
    let (week_row_id, week_name) = match find_current_week_parent(&existing, week) {
        Some(w) => (w.row_id.clone(), w.name.clone().unwrap_or_default()),
        None => {
            let (s, e) = get_week_range(week);
            let name = format!("{} - {}", s.format("%d/%m/%Y"), e.format("%d/%m/%Y"));
            if apply {
                info!("nenhum pai da semana encontrado. Criando: {}...", name);
                let id = lists::create_root_item(&token, &list_id, &name).await?;
                (id, name)
            } else {
                info!("(dry run) criaria o pai da semana: {}", name);
                ("DRY_RUN_ID".to_string(), name)
            }
        }
    };

    println!(
        "\nSemana: {} ({})\nModo: {}\n",
        week_name,
        week_row_id,
        if apply { "APLICAR" } else { "DRY RUN" }
    );
    println!("+ Itens ({}), done={}, data={}:", items.len(), done, date);
    for it in &items {
        println!("  + {}", it);
    }

    if !apply {
        println!("\n(dry run — nada foi enviado ao Slack. Use --apply para efetivar.)");
        return Ok(());
    }

    println!("\nAplicando na Slack List...");
    let date_iso = date.format("%Y-%m-%d").to_string();
    for it in &items {
        match lists::create_subtask(&token, &list_id, &week_row_id, it, done, Some(&date_iso)).await {
            Ok(id) => info!("criado (data {}): {} (row_id={})", date_iso, it, id),
            Err(e) => warn!("falha ao criar '{}': {}", it, e),
        }
    }
    println!("Pronto.");
    Ok(())
}

async fn run_todo_sync(
    apply: bool,
    from: chrono::NaiveDate,
    to: chrono::NaiveDate,
    target_week: chrono::NaiveDate,
) -> Result<(), String> {
    let (token, list_id) = slack_env()?;

    let items = lists::fetch_items(&token, &list_id).await?;
    let (week_row_id, week_name) = match find_current_week_parent(&items, target_week) {
        Some(w) => (w.row_id.clone(), w.name.clone().unwrap_or_default()),
        None => {
            let (s, e) = get_week_range(target_week);
            let name = format!("{} - {}", s.format("%d/%m/%Y"), e.format("%d/%m/%Y"));
            if apply {
                info!("nenhum pai da semana atual encontrado. Criando: {}...", name);
                let id = lists::create_root_item(&token, &list_id, &name).await?;
                (id, name)
            } else {
                info!("(dry run) nenhum pai da semana atual encontrado. Criaria: {}", name);
                ("DRY_RUN_ID".to_string(), name)
            }
        }
    };

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

    let snapshot = tokio::task::spawn_blocking(move || -> Result<analyzer::ActivitySnapshot, String> {
        let (_p, conn) = open_db()?;
        Ok(analyzer::collect_activity_in_range(&conn, from, to))
    })
    .await
    .map_err(|e| format!("spawn_blocking falhou: {}", e))??;

    // Slack Lists impõem 50 subtarefas/parent. Calcula slots disponíveis ANTES
    // de chamar o LLM pra ele já gerar dentro do limite (e como salvaguarda
    // depois, trunca caso ultrapasse).
    const MAX_SUBTASKS: usize = 50;
    let available_slots = MAX_SUBTASKS.saturating_sub(current_subtasks.len());
    let mut plan = analyzer::generate_sync_plan(
        &snapshot,
        &current_subtasks,
        available_slots,
    )
    .await?;

    if plan.novos.len() > available_slots {
        warn!(
            "limite de {} subtarefas/parent atingido: LLM devolveu {} itens, truncando para {} disponíveis",
            MAX_SUBTASKS,
            plan.novos.len(),
            available_slots
        );
        plan.novos.truncate(available_slots);
    }
    if available_slots == 0 && !plan.novos.is_empty() {
        warn!(
            "parent já tem {} subtarefas (limite Slack); nenhum item novo será criado",
            current_subtasks.len()
        );
    }

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
        println!("  + [{}] {}", n.data, n.texto);
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
    let fallback_date = snapshot.today;
    for item in &plan.novos {
        // Valida data e clampa ao range [from, to] pra evitar LLM colocar
        // data fora do período sincronizado.
        let parsed = chrono::NaiveDate::parse_from_str(&item.data, "%Y-%m-%d")
            .unwrap_or(fallback_date);
        let item_date = parsed.max(from).min(to);
        let date_iso = item_date.format("%Y-%m-%d").to_string();
        match lists::create_subtask(
            &token,
            &list_id,
            &week_row_id,
            &item.texto,
            true,
            Some(&date_iso),
        )
        .await
        {
            Ok(id) => info!("criado (feito, data {}): {} (row_id={})", date_iso, item.texto, id),
            Err(e) => warn!("falha ao criar '{}': {}", item.texto, e),
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
    let _ = dotenvy::dotenv();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let cli = Cli::parse();
    let result = match cli.command {
        CliCommand::Start => run_start().await,
        CliCommand::Report { send } => run_report(send).await,
        CliCommand::Todo { action } => match action {
            TodoAction::Inspect => run_todo_inspect().await,
            TodoAction::Add { week, date, done, apply } => {
                let pd = |s: &str| {
                    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
                        .map_err(|_| format!("data inválida: {}", s))
                };
                match pd(&week) {
                    Ok(w) => match date.as_deref().map(pd).transpose() {
                        Ok(d) => run_todo_add(w, d.unwrap_or(w), done, apply).await,
                        Err(e) => Err(e),
                    },
                    Err(e) => Err(e),
                }
            }
            TodoAction::Consolidate { week, apply } => {
                match chrono::NaiveDate::parse_from_str(&week, "%Y-%m-%d") {
                    Ok(d) => run_todo_consolidate(apply, d).await,
                    Err(_) => Err(format!("data inválida em --week: {}", week)),
                }
            }
            TodoAction::Sync { apply, date, from, to, target_week } => {
                let parse_date = |s: String| {
                    chrono::NaiveDate::parse_from_str(&s, "%Y-%m-%d")
                        .map_err(|_| format!("data inválida: {}", s))
                };
                let resolve = || -> Result<(chrono::NaiveDate, chrono::NaiveDate, Option<chrono::NaiveDate>), String> {
                    let today = chrono::Local::now().date_naive();
                    if date.is_some() && (from.is_some() || to.is_some()) {
                        return Err("use --date OU --from/--to, não os dois".to_string());
                    }
                    let (f, t) = if let Some(d) = date.clone() {
                        let p = parse_date(d)?;
                        (p, p)
                    } else {
                        let f = match from.clone() {
                            None => today,
                            Some(s) => parse_date(s)?,
                        };
                        let t = match to.clone() {
                            None => today,
                            Some(s) => parse_date(s)?,
                        };
                        if f > t {
                            return Err(format!("--from {} é depois de --to {}", f, t));
                        }
                        (f, t)
                    };
                    let tw = match target_week.clone() {
                        None => None,
                        Some(s) => Some(parse_date(s)?),
                    };
                    Ok((f, t, tw))
                };
                match resolve() {
                    Ok((f, t, Some(tw))) => {
                        // --target-week explícito: força tudo num só parent (override).
                        run_todo_sync(apply, f, t, tw).await
                    }
                    Ok((f, t, None)) => {
                        // Sem --target-week: divide por semana pra cada chunk
                        // ir pro parent certo.
                        let chunks = split_range_by_week(f, t);
                        if chunks.len() > 1 {
                            info!(
                                "range {} → {} cobre {} semanas, dividindo em chunks",
                                f, t, chunks.len()
                            );
                        }
                        let mut last_err: Option<String> = None;
                        for (cf, ct) in chunks {
                            if let Err(e) = run_todo_sync(apply, cf, ct, ct).await {
                                last_err = Some(format!("chunk {} → {}: {}", cf, ct, e));
                                break;
                            }
                        }
                        match last_err {
                            Some(e) => Err(e),
                            None => Ok(()),
                        }
                    }
                    Err(e) => Err(e),
                }
            }
        },
    };

    if let Err(e) = result {
        error!("{}", e);
        std::process::exit(1);
    }
}
