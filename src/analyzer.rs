use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, UNIX_EPOCH};

use chrono::{Local, NaiveDate};
use log::{debug, warn};
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::lists::ListItem;

const POLL_INTERVAL_SECS: i64 = 60;
const SYSTEM_PROMPT: &str = "Você é um assistente técnico. Baseado neste log do Git e no tempo de tela, gere uma lista de tarefas curta e direta em bullet points do que foi desenvolvido hoje, pronta para ser enviada no Slack de uma equipe de engenharia. Sem introduções, apenas os bullet points.";

#[derive(Debug, Clone)]
pub struct ProjectTime {
    pub nome_do_projeto: String,
    pub minutos: i64,
}

#[derive(Debug, Clone)]
pub struct ProjectGitSummary {
    pub nome_do_projeto: String,
    pub diretorio: Option<PathBuf>,
    pub minutos: i64,
    pub git_status: String,
    pub git_diff_stat: String,
    pub git_log: String,
}

pub fn top_projects_today(conn: &Connection) -> rusqlite::Result<Vec<ProjectTime>> {
    let today: NaiveDate = Local::now().date_naive();
    let prefix = today.format("%Y-%m-%d").to_string();

    let mut stmt = conn.prepare(
        "SELECT nome_do_projeto, COUNT(*) as amostras
         FROM activity_log
         WHERE nome_do_projeto IS NOT NULL
           AND substr(timestamp, 1, 10) = ?1
         GROUP BY nome_do_projeto
         ORDER BY amostras DESC",
    )?;

    let rows = stmt.query_map([&prefix], |row| {
        let nome: String = row.get(0)?;
        let amostras: i64 = row.get(1)?;
        Ok(ProjectTime {
            nome_do_projeto: nome,
            minutos: (amostras * POLL_INTERVAL_SECS) / 60,
        })
    })?;

    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

fn candidate_search_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = dirs::home_dir() {
        roots.push(home.clone());
        for sub in [
            "Projects",
            "projects",
            "dev",
            "Dev",
            "code",
            "Code",
            "workspace",
            "Documents",
            "Área de trabalho",
            "Desktop",
        ] {
            roots.push(home.join(sub));
        }
    }
    roots
}

fn extract_project_token(nome_do_projeto: &str) -> String {
    let after_colon = nome_do_projeto
        .split_once(':')
        .map(|(_, r)| r)
        .unwrap_or(nome_do_projeto)
        .trim();

    let token = after_colon
        .split(|c: char| matches!(c, '—' | '–' | '-' | '/' | '\\' | '|'))
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .last()
        .unwrap_or(after_colon);

    token.trim_start_matches('●').trim().to_string()
}

fn find_git_dir_for(nome_do_projeto: &str) -> Option<PathBuf> {
    let token = extract_project_token(nome_do_projeto);
    if token.is_empty() {
        return None;
    }
    let token_lower = token.to_lowercase();

    for root in candidate_search_roots() {
        let Ok(entries) = fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name.to_lowercase() == token_lower && path.join(".git").exists() {
                return Some(path);
            }
        }
    }
    None
}

fn run_git(dir: &Path, args: &[&str]) -> String {
    match Command::new("git").args(args).current_dir(dir).output() {
        Ok(out) if out.status.success() => {
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            format!("(git {:?} falhou: {})", args, stderr.trim())
        }
        Err(e) => format!("(falha ao executar git {:?}: {})", args, e),
    }
}

pub fn collect_git_summaries(projects: &[ProjectTime]) -> Vec<ProjectGitSummary> {
    let mut seen: HashMap<PathBuf, ProjectGitSummary> = HashMap::new();
    let mut out: Vec<ProjectGitSummary> = Vec::new();

    for p in projects {
        let dir = find_git_dir_for(&p.nome_do_projeto);
        match &dir {
            Some(d) => {
                if let Some(existing) = seen.get_mut(d) {
                    existing.minutos += p.minutos;
                    continue;
                }
                let summary = ProjectGitSummary {
                    nome_do_projeto: p.nome_do_projeto.clone(),
                    diretorio: Some(d.clone()),
                    minutos: p.minutos,
                    git_status: run_git(d, &["status", "--short"]),
                    git_diff_stat: run_git(d, &["diff", "--stat"]),
                    git_log: run_git(
                        d,
                        &["log", "--since=6am", "--pretty=format:%h %s", "--all"],
                    ),
                };
                seen.insert(d.clone(), summary.clone());
                out.push(summary);
            }
            None => {
                debug!(
                    "projeto sem diretório git identificado: {}",
                    p.nome_do_projeto
                );
                out.push(ProjectGitSummary {
                    nome_do_projeto: p.nome_do_projeto.clone(),
                    diretorio: None,
                    minutos: p.minutos,
                    git_status: String::new(),
                    git_diff_stat: String::new(),
                    git_log: String::new(),
                });
            }
        }
    }
    out
}

fn build_user_payload(summaries: &[ProjectGitSummary]) -> String {
    let mut buf = String::new();
    buf.push_str("# Tempo de tela por projeto (hoje)\n");
    for s in summaries {
        buf.push_str(&format!(
            "- {} — {} min{}\n",
            s.nome_do_projeto,
            s.minutos,
            s.diretorio
                .as_ref()
                .map(|p| format!(" [{}]", p.display()))
                .unwrap_or_default()
        ));
    }

    buf.push_str("\n# Resumo bruto do Git por projeto\n");
    for s in summaries {
        if s.diretorio.is_none() {
            continue;
        }
        buf.push_str(&format!("\n## {}\n", s.nome_do_projeto));
        buf.push_str("### git status --short\n");
        buf.push_str(if s.git_status.is_empty() {
            "(vazio)"
        } else {
            &s.git_status
        });
        buf.push_str("\n### git diff --stat\n");
        buf.push_str(if s.git_diff_stat.is_empty() {
            "(vazio)"
        } else {
            &s.git_diff_stat
        });
        buf.push_str("\n### git log --since=6am\n");
        buf.push_str(if s.git_log.is_empty() {
            "(vazio)"
        } else {
            &s.git_log
        });
        buf.push('\n');
    }
    buf
}

#[derive(Debug, Clone, Copy)]
pub enum LlmProvider {
    OpenAi,
    Ollama,
    Claude,
}

fn detect_provider() -> LlmProvider {
    match env::var("SLACK_TRACKER_LLM").ok().as_deref() {
        Some("ollama") => LlmProvider::Ollama,
        Some("openai") => LlmProvider::OpenAi,
        Some("claude") | Some("anthropic") => LlmProvider::Claude,
        _ => {
            if env::var("ANTHROPIC_API_KEY").is_ok() {
                LlmProvider::Claude
            } else if env::var("OPENAI_API_KEY").is_ok() {
                LlmProvider::OpenAi
            } else {
                LlmProvider::Ollama
            }
        }
    }
}

fn call_openai(user_payload: &str) -> Result<String, String> {
    let api_key = env::var("OPENAI_API_KEY")
        .map_err(|_| "OPENAI_API_KEY não definido".to_string())?;
    let model = env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_string());
    let url = env::var("OPENAI_BASE_URL")
        .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());

    let body = json!({
        "model": model,
        "messages": [
            {"role": "system", "content": SYSTEM_PROMPT},
            {"role": "user", "content": user_payload},
        ],
        "temperature": 0.2,
    });

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| format!("erro ao construir client: {}", e))?;

    let resp = client
        .post(format!("{}/chat/completions", url.trim_end_matches('/')))
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .map_err(|e| format!("erro na request OpenAI: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().unwrap_or_default();
        return Err(format!("OpenAI {}: {}", status, text));
    }

    let v: Value = resp.json().map_err(|e| format!("json inválido: {}", e))?;
    v["choices"][0]["message"]["content"]
        .as_str()
        .map(|s| s.trim().to_string())
        .ok_or_else(|| format!("resposta OpenAI sem conteúdo: {}", v))
}

fn call_ollama(user_payload: &str) -> Result<String, String> {
    let base = env::var("OLLAMA_BASE_URL")
        .unwrap_or_else(|_| "http://localhost:11434".to_string());
    let model = env::var("OLLAMA_MODEL").unwrap_or_else(|_| "llama3.1".to_string());

    let body = json!({
        "model": model,
        "stream": false,
        "messages": [
            {"role": "system", "content": SYSTEM_PROMPT},
            {"role": "user", "content": user_payload},
        ],
        "options": {"temperature": 0.2},
    });

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| format!("erro ao construir client: {}", e))?;

    let resp = client
        .post(format!("{}/api/chat", base.trim_end_matches('/')))
        .json(&body)
        .send()
        .map_err(|e| format!("erro na request Ollama: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().unwrap_or_default();
        return Err(format!("Ollama {}: {}", status, text));
    }

    let v: Value = resp.json().map_err(|e| format!("json inválido: {}", e))?;
    v["message"]["content"]
        .as_str()
        .map(|s| s.trim().to_string())
        .ok_or_else(|| format!("resposta Ollama sem conteúdo: {}", v))
}

pub fn generate_daily_summary(conn: &Connection) -> Result<String, String> {
    let projects = top_projects_today(conn)
        .map_err(|e| format!("erro ao consultar sqlite: {}", e))?;

    if projects.is_empty() {
        return Ok("(nenhuma atividade registrada hoje)".to_string());
    }

    let summaries = collect_git_summaries(&projects);
    let payload = build_user_payload(&summaries);
    debug!("payload para LLM:\n{}", payload);

    let provider = detect_provider();
    let result = match provider {
        LlmProvider::OpenAi => call_openai(&payload),
        LlmProvider::Ollama => call_ollama(&payload),
        LlmProvider::Claude => Err(
            "o comando `report` não suporta Claude — use `todo sync` ou troque SLACK_TRACKER_LLM"
                .to_string(),
        ),
    };

    match result {
        Ok(s) => Ok(s),
        Err(e) => {
            warn!("falha na chamada LLM ({:?}): {}", provider, e);
            Err(e)
        }
    }
}

// ══════════════════════════════════════════════════════════════════════
//  Coleta rica de atividade + sincronização da Slack List (modo Todo)
// ══════════════════════════════════════════════════════════════════════

const SYSTEM_PROMPT_TODO: &str = "Você é uma engenheira de software brasileira atualizando a sua lista de tarefas da semana no Slack no fim do dia. O propósito desta lista é que o GESTOR DE EQUIPE (que entende um pouco de programação, mas não é dev full-time) consiga ler de relance e entender o que você fez no dia. Ele vai bater o olho na lista — então cada item precisa ser claro, curto e focado no RESULTADO, não no meio técnico.\n\nSua tarefa: ler a atividade bruta do dia (commits, diffs, arquivos modificados, tempo de tela) e a lista atual da semana, e retornar APENAS um JSON (sem markdown, sem texto antes/depois) com os itens a adicionar e os row_ids a marcar como feitos.\n\n══════════════════════════════════════════════════\nCOMO ESCREVER OS ITENS (crítico)\n══════════════════════════════════════════════════\n\nTAMANHO: cada item deve ser CURTO e ATÔMICO. Idealmente 5 a 10 palavras, no máximo 15. Uma frase só, sem \"e\"/\"além disso\"/\",\" emendando várias coisas. Um item = UMA coisa.\n\nSIM — itens atômicos e curtos:\n  ✓ \"Implementei a tela de verificação de SSL no app\"\n  ✓ \"Adicionei suporte a SSL no agente do servidor\"\n  ✓ \"Expandi a documentação de arquitetura\"\n  ✓ \"Atualizei o setup do projeto\"\n  ✓ \"Comecei a base do projeto\"\n\nNÃO — itens longos/combinados:\n  ✗ \"Avancei no projeto: implementei a tela de SSL, adicionei suporte no agente e expandi toda a documentação (API, arquitetura, setup)\"   (longo demais, bundled)\n  ✗ \"Trabalhei na camada de API e também nos docs e também no frontend\"   (várias coisas num item só)\n  ✗ \"Atualizar handlers.rs\"                    (nome de arquivo bruto)\n  ✗ \"Mexi no código\"                           (genérico demais)\n  ✗ \"commit abc1234: fix bug\"                  (parecendo log de git)\n\nREGRA PRÁTICA: se um item usa 2 ou mais \"e\"/\",\" conectando ações diferentes, QUEBRE em itens separados. Melhor 6 itens atômicos de 8 palavras do que 2 itens longos de 30 palavras.\n\n══════════════════════════════════════════════════\nREGRAS OBRIGATÓRIAS\n══════════════════════════════════════════════════\n\n1. PORTUGUÊS BRASILEIRO, primeira pessoa, tom natural. Commits em inglês você TRADUZ e reescreve, não copia.\n2. MÁXIMO 10 itens novos. Prefira itens curtos e atômicos — é melhor ter 6-8 itens pequenos do que 2-3 longos. Só NÃO ultrapasse 10.\n3. NUNCA DUPLIQUE: se algo que você fez hoje já existe na lista da semana, coloque o row_id em \"marcar_feito\" e NÃO crie um novo item com texto parecido.\n4. NUNCA invente tarefas que não estejam na atividade bruta. Se a atividade está vazia para um projeto, não gere itens dele.\n5. Foco em RESULTADO/ENTREGA, não em arquivo ou comando. O gestor quer saber o QUE foi feito, não ONDE no código.\n6. Não mencione timestamps, minutos, paths, nomes de commit, hash, linhas de código, row_ids dentro do texto dos itens.\n7. Não use jargão desnecessário. Termos como \"API\", \"frontend\", \"banco\" são OK. Termos como \"middleware\", \"handler\", \"struct\" são OK se vierem no contexto de uma feature concreta, não isolados.\n8. Se não há atividade real hoje, retorne novos=[] e marcar_feito=[].\n\n══════════════════════════════════════════════════\nFORMATO DE SAÍDA\n══════════════════════════════════════════════════\n\nRetorne APENAS este JSON, nada mais:\n{\"novos\": [\"texto\", \"texto\", ...], \"marcar_feito\": [\"Rec...\", \"Rec...\"]}";

#[derive(Debug, Clone)]
pub struct ProjectActivity {
    pub dir: PathBuf,
    pub minutos_tela: i64,
    pub git_log: String,
    pub git_diff_stat: String,
    pub git_status: String,
    pub arquivos_modificados: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ActivitySnapshot {
    pub today: NaiveDate,
    pub projetos: Vec<ProjectActivity>,
    pub top_janelas: Vec<(String, i64)>,
}

#[derive(Debug, Deserialize)]
pub struct SyncPlan {
    #[serde(default)]
    pub novos: Vec<String>,
    #[serde(default)]
    pub marcar_feito: Vec<String>,
}

fn project_roots() -> Vec<PathBuf> {
    env::var("PROJECT_ROOTS")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .collect()
}

fn minutes_by_window_today(conn: &Connection) -> rusqlite::Result<Vec<(String, i64)>> {
    let prefix = Local::now().date_naive().format("%Y-%m-%d").to_string();
    let mut stmt = conn.prepare(
        "SELECT nome_da_janela, COUNT(*) as amostras
         FROM activity_log
         WHERE substr(timestamp, 1, 10) = ?1
         GROUP BY nome_da_janela
         ORDER BY amostras DESC
         LIMIT 15",
    )?;
    let rows = stmt.query_map([&prefix], |row| {
        let nome: String = row.get(0)?;
        let amostras: i64 = row.get(1)?;
        Ok((nome, (amostras * POLL_INTERVAL_SECS) / 60))
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

fn minutes_for_dir_today(conn: &Connection, dir: &Path) -> i64 {
    let dir_name = match dir.file_name().and_then(|n| n.to_str()) {
        Some(n) => n.to_lowercase(),
        None => return 0,
    };
    let prefix = Local::now().date_naive().format("%Y-%m-%d").to_string();
    let pattern = format!("%{}%", dir_name);
    let mut stmt = match conn.prepare(
        "SELECT COUNT(*) FROM activity_log
         WHERE substr(timestamp, 1, 10) = ?1
           AND lower(nome_da_janela) LIKE ?2",
    ) {
        Ok(s) => s,
        Err(_) => return 0,
    };
    let amostras: i64 = stmt
        .query_row([&prefix, &pattern], |row| row.get(0))
        .unwrap_or(0);
    (amostras * POLL_INTERVAL_SECS) / 60
}

fn files_modified_today(root: &Path) -> Vec<String> {
    let today_midnight = Local::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|dt| dt.and_local_timezone(Local).single());
    let cutoff = match today_midnight {
        Some(dt) => dt.timestamp() as u64,
        None => return Vec::new(),
    };

    let mut out: Vec<String> = Vec::new();
    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
    const SKIP: &[&str] = &[
        ".git",
        "target",
        "node_modules",
        ".next",
        "dist",
        "build",
        ".venv",
        "venv",
        "__pycache__",
        ".cache",
    ];
    const MAX: usize = 200;

    while let Some(dir) = stack.pop() {
        if out.len() >= MAX {
            break;
        }
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n,
                None => continue,
            };
            if name.starts_with('.') && name != ".env" {
                continue;
            }
            if SKIP.contains(&name) {
                continue;
            }
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            if meta.is_dir() {
                stack.push(path);
                continue;
            }
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            if mtime >= cutoff {
                if let Ok(rel) = path.strip_prefix(root) {
                    out.push(rel.display().to_string());
                }
                if out.len() >= MAX {
                    break;
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn is_git_root(dir: &Path) -> bool {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(dir)
        .output();
    let Ok(out) = output else { return false };
    if !out.status.success() {
        return false;
    }
    let toplevel = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let Ok(canon_dir) = dir.canonicalize() else {
        return false;
    };
    let Ok(canon_top) = Path::new(&toplevel).canonicalize() else {
        return false;
    };
    canon_dir == canon_top
}

pub fn collect_activity_today(conn: &Connection) -> ActivitySnapshot {
    let mut projetos = Vec::new();
    for root in project_roots() {
        if !root.exists() {
            warn!("PROJECT_ROOT não existe, pulando: {}", root.display());
            continue;
        }
        let root_is_git = is_git_root(&root);
        if !root_is_git {
            debug!(
                "{} não é a raiz de um repo git — pulando comandos git, usando só mtime",
                root.display()
            );
        }
        let git_log = if root_is_git {
            run_git(
                &root,
                &["log", "--since=midnight", "--pretty=format:%h %s", "--all"],
            )
        } else {
            String::new()
        };
        let git_diff_stat = if root_is_git {
            run_git(&root, &["diff", "--stat"])
        } else {
            String::new()
        };
        let git_status = if root_is_git {
            run_git(&root, &["status", "--short"])
        } else {
            String::new()
        };
        let arquivos_modificados = files_modified_today(&root);
        let minutos_tela = minutes_for_dir_today(conn, &root);

        projetos.push(ProjectActivity {
            dir: root,
            minutos_tela,
            git_log,
            git_diff_stat,
            git_status,
            arquivos_modificados,
        });
    }

    let top_janelas = minutes_by_window_today(conn).unwrap_or_default();

    ActivitySnapshot {
        today: Local::now().date_naive(),
        projetos,
        top_janelas,
    }
}

fn build_todo_payload(snapshot: &ActivitySnapshot, list_items: &[ListItem]) -> String {
    let mut buf = String::new();
    buf.push_str(&format!("# Data de hoje: {}\n\n", snapshot.today));

    buf.push_str("# Atividade real de hoje\n\n");
    for p in &snapshot.projetos {
        let nome = p
            .dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("(sem nome)");
        buf.push_str(&format!(
            "## Projeto: {} ({} min de tela hoje)\n",
            nome, p.minutos_tela
        ));
        buf.push_str("### git log desde 00:00 (pode ter commits em inglês)\n");
        buf.push_str(if p.git_log.trim().is_empty() {
            "(sem commits hoje)"
        } else {
            &p.git_log
        });
        buf.push_str("\n### git status (não commitado)\n");
        buf.push_str(if p.git_status.trim().is_empty() {
            "(working tree limpo)"
        } else {
            &p.git_status
        });
        buf.push_str("\n### git diff --stat\n");
        buf.push_str(if p.git_diff_stat.trim().is_empty() {
            "(sem diffs)"
        } else {
            &p.git_diff_stat
        });
        buf.push_str("\n### Arquivos modificados hoje no disco\n");
        if p.arquivos_modificados.is_empty() {
            buf.push_str("(nenhum)\n");
        } else {
            for f in p.arquivos_modificados.iter().take(30) {
                buf.push_str(&format!("- {}\n", f));
            }
        }
        buf.push('\n');
    }

    buf.push_str("# Top janelas ativas hoje (sinal do que estava na tela)\n");
    for (w, m) in snapshot.top_janelas.iter().take(10) {
        buf.push_str(&format!("- {} min — {}\n", m, w));
    }

    buf.push_str("\n# Lista atual da semana no Slack (NÃO duplique esses itens)\n");
    buf.push_str("Subtarefas que já existem sob a semana atual:\n");
    for it in list_items {
        let status = if it.done { "[x]" } else { "[ ]" };
        let name = it.name.as_deref().unwrap_or("(sem nome)");
        buf.push_str(&format!("- {} {} (row_id={})\n", status, name, it.row_id));
    }

    buf.push_str("\n# Sua tarefa\nRetorne o JSON com no máximo 10 itens em `novos` e qualquer row_id cuja descrição bate com algo que eu realmente fiz hoje em `marcar_feito`. Nada mais.\n");
    buf
}

fn parse_sync_plan(raw: &str) -> Result<SyncPlan, String> {
    let trimmed = raw.trim();
    let start = trimmed.find('{').ok_or("resposta sem JSON")?;
    let end = trimmed.rfind('}').ok_or("resposta sem JSON fechado")?;
    let slice = &trimmed[start..=end];
    serde_json::from_str(slice).map_err(|e| format!("parse do JSON do LLM falhou: {}", e))
}

async fn call_claude_async(user_payload: &str, system: &str) -> Result<String, String> {
    let key = env::var("ANTHROPIC_API_KEY")
        .map_err(|_| "ANTHROPIC_API_KEY não definido".to_string())?;
    let model = env::var("ANTHROPIC_MODEL").unwrap_or_else(|_| "claude-sonnet-4-6".to_string());
    let base =
        env::var("ANTHROPIC_BASE_URL").unwrap_or_else(|_| "https://api.anthropic.com".to_string());

    let body = json!({
        "model": model,
        "max_tokens": 2048,
        "temperature": 0.2,
        "system": [{
            "type": "text",
            "text": system,
            "cache_control": {"type": "ephemeral"}
        }],
        "messages": [{
            "role": "user",
            "content": user_payload
        }]
    });

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| format!("erro ao construir client: {}", e))?;

    let resp = client
        .post(format!("{}/v1/messages", base.trim_end_matches('/')))
        .header("x-api-key", key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("erro na request Claude: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("Claude {}: {}", status, text));
    }

    let v: Value = resp
        .json()
        .await
        .map_err(|e| format!("json inválido: {}", e))?;

    if let Some(usage) = v.get("usage") {
        debug!("claude usage: {}", usage);
    }

    v["content"][0]["text"]
        .as_str()
        .map(|s| s.trim().to_string())
        .ok_or_else(|| format!("resposta Claude sem conteúdo: {}", v))
}

async fn call_ollama_async(user_payload: &str, system: &str) -> Result<String, String> {
    let base = env::var("OLLAMA_BASE_URL")
        .unwrap_or_else(|_| "http://localhost:11434".to_string());
    let model = env::var("OLLAMA_MODEL").unwrap_or_else(|_| "llama3.1".to_string());

    let body = json!({
        "model": model,
        "stream": false,
        "format": "json",
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user_payload},
        ],
        "options": {"temperature": 0.2},
    });

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|e| format!("erro ao construir client: {}", e))?;

    let resp = client
        .post(format!("{}/api/chat", base.trim_end_matches('/')))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("erro na request Ollama: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("Ollama {}: {}", status, text));
    }

    let v: Value = resp
        .json()
        .await
        .map_err(|e| format!("json inválido: {}", e))?;
    v["message"]["content"]
        .as_str()
        .map(|s| s.trim().to_string())
        .ok_or_else(|| format!("resposta Ollama sem conteúdo: {}", v))
}

pub async fn generate_sync_plan(
    snapshot: &ActivitySnapshot,
    current_week_items: &[ListItem],
) -> Result<SyncPlan, String> {
    let payload = build_todo_payload(snapshot, current_week_items);
    debug!("payload para LLM (todo sync):\n{}", payload);
    let provider = detect_provider();
    debug!("provider selecionado: {:?}", provider);
    let raw = match provider {
        LlmProvider::Claude => call_claude_async(&payload, SYSTEM_PROMPT_TODO).await?,
        LlmProvider::Ollama => call_ollama_async(&payload, SYSTEM_PROMPT_TODO).await?,
        LlmProvider::OpenAi => {
            return Err("provider OpenAI ainda não suportado no fluxo todo sync".to_string());
        }
    };
    debug!("resposta bruta do LLM:\n{}", raw);
    let mut plan = parse_sync_plan(&raw)?;
    if plan.novos.len() > 10 {
        warn!(
            "LLM devolveu {} itens, truncando para 10",
            plan.novos.len()
        );
        plan.novos.truncate(10);
    }
    let existing_row_ids: std::collections::HashSet<&str> =
        current_week_items.iter().map(|i| i.row_id.as_str()).collect();
    plan.marcar_feito
        .retain(|r| existing_row_ids.contains(r.as_str()));
    Ok(plan)
}
