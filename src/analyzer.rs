use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, UNIX_EPOCH};

use chrono::{Local, NaiveDate};
use log::{debug, info, warn};
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::lists::ListItem;

const POLL_INTERVAL_SECS: i64 = 60;
const SYSTEM_PROMPT: &str = "Você é uma engenheira de software sênior gerando um relatório diário para seu gestor de equipe. Baseado no git log, diffs e tempo de tela, extraia o que realmente foi desenvolvido hoje em bullet points concisos, prontos para Slack.\n\nREGRAS:\n1. Cada bullet point: 5-10 palavras, uma ação clara, resultado visível. Sem \"trabalhei em\" ou \"avancei\" — use: Automatizei / Mitiguei / Blindei / Orquestrei / Padronizei / Acelerei / Escalei / Reforcei / Eliminei / Isolei / Separei / Centralizei / Extraí / Migrei / Substituí / Removi / Conectei / Corrigi / Criei / Documentei.\n2. IMPACTO, não técnica: \"Reduzi falsos positivos isolando IPs locais\" (mostra benefício) em vez de \"Adicionei filtro de IPs locais\" (só diz o que fez).\n3. Bugs/erros = vulnerabilidades que você SELOU. Diga \"Blindei o sistema contra falhas de leitura\" não \"Corrigi erro de arquivo\".\n4. Sem introduções, sem conclusões — APENAS bullet points.\n5. Traduza commits em inglês para português com a regra de impacto.\n6. Se não houve atividade, retorne vazio (sem \"sem atividade hoje\").";

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

pub fn top_projects_today(conn: &Connection, date: NaiveDate) -> rusqlite::Result<Vec<ProjectTime>> {
    top_projects_in_range(conn, date, date)
}

pub fn top_projects_in_range(
    conn: &Connection,
    from: NaiveDate,
    to: NaiveDate,
) -> rusqlite::Result<Vec<ProjectTime>> {
    let from_s = from.format("%Y-%m-%d").to_string();
    let to_s = to.format("%Y-%m-%d").to_string();

    let mut stmt = conn.prepare(
        "SELECT nome_do_projeto, COUNT(*) as amostras
         FROM activity_log
         WHERE nome_do_projeto IS NOT NULL
           AND substr(timestamp, 1, 10) BETWEEN ?1 AND ?2
         GROUP BY nome_do_projeto
         ORDER BY amostras DESC",
    )?;

    let rows = stmt.query_map([&from_s, &to_s], |row| {
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

    // Separa apenas em separadores visuais com espaço ao redor (ex: " - ", " — ")
    // para não quebrar nomes com hífen como "auto-writing" ou "my-project".
    let re = regex::Regex::new(r"\s+[-–—/\\|]\s+").unwrap();
    let parts: Vec<&str> = re.split(after_colon).collect();
    let token = parts.last().copied().unwrap_or(after_colon).trim();

    token.trim_start_matches('●').trim().to_string()
}

fn find_git_dir_for(nome_do_projeto: &str) -> Option<PathBuf> {
    let token = extract_project_token(nome_do_projeto);
    if token.is_empty() {
        return None;
    }
    let token_lower = token.to_lowercase();

    // dirs que não são repos git e servem só de agrupador (ex: plugin/, packages/)
    const SKIP_GROUP: &[&str] = &[
        "target", "node_modules", ".git", ".cache", "dist", "build",
        ".venv", "venv", "__pycache__",
    ];

    for root in candidate_search_roots() {
        let Ok(entries) = fs::read_dir(&root) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() { continue; }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };

            // match direto (1 nível)
            if name.to_lowercase() == token_lower && path.join(".git").exists() {
                return Some(path);
            }

            // subdiretório agrupador sem .git: tenta 1 nível a mais
            if !path.join(".git").exists() && !SKIP_GROUP.contains(&name) {
                let Ok(sub_entries) = fs::read_dir(&path) else { continue };
                for sub in sub_entries.flatten() {
                    let sub_path = sub.path();
                    if !sub_path.is_dir() { continue; }
                    let Some(sub_name) = sub_path.file_name().and_then(|n| n.to_str()) else { continue };
                    if sub_name.to_lowercase() == token_lower && sub_path.join(".git").exists() {
                        return Some(sub_path);
                    }
                }
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
    Gemini,
}

fn detect_provider() -> LlmProvider {
    match env::var("SLACK_TRACKER_LLM").ok().as_deref() {
        Some("ollama") => LlmProvider::Ollama,
        Some("openai") => LlmProvider::OpenAi,
        Some("claude") | Some("anthropic") => LlmProvider::Claude,
        Some("gemini") | Some("google") => LlmProvider::Gemini,
        _ => {
            if env::var("ANTHROPIC_API_KEY").is_ok() {
                LlmProvider::Claude
            } else if env::var("OPENAI_API_KEY").is_ok() {
                LlmProvider::OpenAi
            } else if env::var("GEMINI_API_KEY").is_ok() {
                LlmProvider::Gemini
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
    let projects = top_projects_today(conn, Local::now().date_naive())
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
        LlmProvider::Gemini => Err(
            "o comando `report` não suporta Gemini — use `todo sync` ou troque SLACK_TRACKER_LLM"
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

const SYSTEM_PROMPT_TODO: &str = "Você é uma engenheira de software brasileira atualizando a sua lista de tarefas da semana no Slack no fim do dia. O propósito desta lista é que o GESTOR DE EQUIPE (que entende um pouco de programação, mas não é dev full-time) consiga ler de relance e entender o que você fez no dia. Ele vai bater o olho na lista — então cada item precisa ser claro, curto e focado no RESULTADO, não no meio técnico.\n\nSua tarefa: ler a atividade bruta do dia (commits, diffs, arquivos modificados, tempo de tela) e a lista atual da semana, e retornar APENAS um JSON (sem markdown, sem texto antes/depois) com os itens a adicionar e os row_ids a marcar como feitos.\n\n══════════════════════════════════════════════════\nCOMO ESCREVER OS ITENS (crítico)\n══════════════════════════════════════════════════\n\nTAMANHO: cada item deve ser CURTO e ATÔMICO. Idealmente 5 a 10 palavras, no máximo 15. Uma frase só, sem \"e\"/\"além disso\"/\",\" emendando várias coisas. Um item = UMA coisa.\n\nFORMATO OBRIGATÓRIO: todo item começa com o nome do projeto entre colchetes, seguido da ação em primeira pessoa. O nome do projeto é o nome da pasta que aparece em `## Projeto: <nome>` na atividade. Isso é crítico — sem o prefixo, o gestor não sabe a qual sistema o item se refere.\n\nSIM — itens atômicos, curtos e com prefixo do projeto:\n  ✓ \"[server-controller] Implementei a tela de verificação de SSL no app\"\n  ✓ \"[server-controller] Adicionei suporte a SSL no agente\"\n  ✓ \"[slack-tracker] Documentei a arquitetura de coleta e análise de janelas\"\n  ✓ \"[slack-tracker] Criei a base do projeto com estrutura inicial\"\n  ✓ \"[server-controller] Atualizei o setup do projeto\"\n\nNÃO — itens sem prefixo, longos ou combinados:\n  ✗ \"Implementei a tela de verificação de SSL no app\"   (faltou o prefixo do projeto)\n  ✗ \"[server-controller] implementei a tela de SSL, adicionei suporte no agente e expandi toda a documentação (API, arquitetura, setup)\"   (longo demais, bundled)\n  ✗ \"Trabalhei na camada de API e também nos docs e também no frontend\"   (várias coisas num item só, sem prefixo)\n  ✗ \"Atualizar handlers.rs\"                    (nome de arquivo bruto)\n  ✗ \"Mexi no código\"                           (genérico demais)\n  ✗ \"commit abc1234: fix bug\"                  (parecendo log de git)\n\nREGRA PRÁTICA: se um item usa 2 ou mais \"e\"/\",\" conectando ações diferentes, QUEBRE em itens separados. Melhor 6 itens atômicos de 8 palavras do que 2 itens longos de 30 palavras.\n\n══════════════════════════════════════════════════\nREGRA DO IMPACTO (Tradução de Código para Valor)\n══════════════════════════════════════════════════\n\nO gestor não compra código, ele compra segurança, performance, estabilidade e produtividade. Se o diff mostra uma otimização técnica, seu item deve refletir o impacto gerado.\n\n  ✗ \"[api] Adicionei filtro de IPs de rede local\"              (Apenas diz o que fez)\n  ✓ \"[api] Reduzi alertas de falsos positivos isolando IPs locais\"  (Mostra o benefício)\n  \n  ✗ \"[obscura] Mudei as queries para assíncronas\"\n  ✓ \"[obscura] Acelerei o tempo de resposta com execuções assíncronas\"\n  \n  ✗ \"[backend] Refatorei o módulo de autenticação\"\n  ✓ \"[backend] Eliminei tentativas de login falhadas duplicadas centralizando a autenticação\"\n\nToda mudança tem impacto: segurança (blindagem contra falhas), performance (velocidade), confiabilidade (redução de erros), ou manutenibilidade (padrão/redução cognitiva). Deixe claro qual.\n\n══════════════════════════════════════════════════\nTOM — sênior sem se mostrar (crítico)\n══════════════════════════════════════════════════\n\nEssa lista precisa soar como alguém que sabe o que está fazendo e não precisa provar isso. Tom confiante e direto. A marca de um sênior não é o vocabulário difícil — é a ESPECIFICIDADE. Cada item deixa claro O QUE mudou, não só em qual componente.\n\nREGRA DE OURO: se você escreveu um verbo e o leitor ainda pergunta \"mas o que mudou?\", o item está errado. O sênior sempre diz O QUE fez dentro de X.\n\n  ✗ \"[api] Refatorei o módulo de análise\"              — o que mudou?\n  ✗ \"[plugin] Expandi o gerador de landing pages\"      — expandiu como?\n  ✗ \"[api] Otimizei o fluxo de coleta\"                 — como?\n  ✗ \"[app] Ajustei os templates\"                      — ajustou o quê?\n  ✗ \"[app] Adicionei suporte a múltiplos provedores\"   — quais provedores?\n\n  ✓ \"[api] Separei a coleta de atividade por janela da análise por projeto\"\n  ✓ \"[plugin] Adicionei variáveis dinâmicas por país no gerador de landing pages\"\n  ✓ \"[app] Centralizei as chamadas a OpenAI, Claude e Gemini em um único módulo\"\n  ✓ \"[app] Corrigi flash de R$0 antes dos dados carregarem no dashboard\"\n  ✓ \"[api] Removi paths hardcoded das constantes para variáveis de ambiente\"\n  ✓ \"[infra] Adicionei catch-up automático no scheduler para reboots pós-horário\"\n  ✓ \"[plugin] Ajustei o template de artigo para incluir âncoras de seção\" — diz O QUE ajustou\n\nVERBOS DE ALTO IMPACTO (Use para iniciar os itens — transmitem autoridade e decisão):\n  Automatizei / Mitiguei / Blindei / Orquestrei / Padronizei / Acelerei / Escalei / Reforcei / Eliminei (gargalo/risco) / Isolei / Separei / Centralizei / Extraí / Migrei / Substituí / Removi / Conectei / Corrigi / Criei / Documentei\n\nVERBOS QUE DEVEM SUMIR (genéricos — qualquer nível usa, não dizem nada sozinhos):\n  Tentei / Mexi / Trabalhei em / Avancei / Fiz melhorias / Expandi / Refatorei / Otimizei / Ajustei\n  (\"Expandi X\" → diga como. \"Refatorei X\" → diga o que mudou. \"Otimizei X\" → diga o que melhorou.)\n\nSEM JARGÃO PRA IMPRESSIONAR: use o técnico quando comunica algo concreto. Um gestor que entende um pouco de programação vai entender \"módulo\", \"endpoint\", \"banco\" — mas não empilhe buzzwords.\n  ✓ \"[app] Corrigi flash de valores zerados no carregamento do dashboard\"\n  ✗ \"[app] Resolvi race condition no lifecycle de hidratação do estado assíncrono\"\n\nSEM MODÉSTIA FALSA E SEM EXAGERO:\n  ✓ \"[app] Padronizei os estados de loading em todas as páginas\"\n  ✗ \"[app] Fiz um pequeno ajuste nos loadings\"             (menor do que é)\n  ✗ \"[app] Redesenhei toda a arquitetura de UX do sistema\" (exagero)\n\n══════════════════════════════════════════════════\nREGRAS OBRIGATÓRIAS\n══════════════════════════════════════════════════\n\n1. PORTUGUÊS BRASILEIRO, primeira pessoa, tom assertivo de sênior. Commits em inglês você TRADUZ e reescreve, não copia.\n1b. TODO item começa com `[nome-do-projeto]` (o nome da pasta que aparece em `## Projeto:` na atividade bruta). Se a atividade do dia envolveu mais de um projeto, misture itens com prefixos diferentes.\n2. Liste os trabalhos distintos da atividade bruta RESPEITANDO o limite duro de slots informado no payload do usuário (Slack List tem máximo 50 subtarefas/parent — o payload diz quantos sobram). Se houver mais trabalhos do que slots, NÃO descarte itens — CONSOLIDE: agrupe 2-3 mudanças relacionadas/pequenas em um item só (15-20 palavras OK quando consolidado), de forma que tudo de relevante seja mencionado. Exemplo: \"[api] Atualizei fastapi, starlette, python-jose e python-dotenv corrigindo CVEs\" no lugar de 4 itens separados. Prefira itens atômicos QUANDO HÁ ESPAÇO; consolide só quando precisa caber. Nunca deixe trabalho real fora da lista.\n3. NUNCA DUPLIQUE: se algo que você fez hoje já existe na lista da semana, coloque o row_id em \"marcar_feito\" e NÃO crie um novo item com texto parecido.\n4. NUNCA invente tarefas que não estejam na atividade bruta. Se a atividade está vazia para um projeto, não gere itens dele.\n5. Foco em RESULTADO/ENTREGA, não em arquivo ou comando. O gestor quer saber o QUE foi feito, não ONDE no código.\n6. Não mencione timestamps, minutos, paths, nomes de commit, hash, linhas de código, row_ids dentro do texto dos itens.\n7. Não use jargão desnecessário. Termos como \"API\", \"frontend\", \"banco\" são OK quando comunicam algo concreto. Nunca empilhe buzzwords.\n8. POSTURA DE PROTEÇÃO: Quando a atividade envolver correções de bugs, tratamentos de erro (Result em Rust, try/catch) ou firewalls, use termos que transmitam estabilidade para a empresa. Em vez de \"Corrigi o erro de arquivo não encontrado\", use \"Blindei o sistema contra falhas de leitura de arquivos\". Bugs = vulnerabilidades que você selou. Tratamentos de erro = defesas que você ativou.\n9. Se não há atividade real hoje, retorne novos=[] e marcar_feito=[].\n\n══════════════════════════════════════════════════\nFORMATO DE SAÍDA\n══════════════════════════════════════════════════\n\nRetorne APENAS este JSON, nada mais:\n{\"novos\": [{\"texto\": \"[projeto] descrição\", \"data\": \"YYYY-MM-DD\"}, ...], \"marcar_feito\": [\"Rec...\", \"Rec...\"]}\n\nA `data` de cada item DEVE refletir o dia REAL em que aquele trabalho foi feito (use a data do commit no git log, ou o mtime do arquivo modificado). Para itens que agrupam múltiplos commits/dias, use a data mais recente. Nunca chute 'hoje' se há sinal melhor.";

#[derive(Debug, Clone)]
pub struct ProjectActivity {
    pub dir: PathBuf,
    pub minutos_tela: i64,
    pub git_log: String,
    pub git_diff_stat: String,
    pub git_status: String,
    pub arquivos_modificados: Vec<(String, NaiveDate)>,
}

#[derive(Debug, Clone)]
pub struct ActivitySnapshot {
    pub today: NaiveDate,
    pub projetos: Vec<ProjectActivity>,
    pub top_janelas: Vec<(String, i64)>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct PlannedItem {
    pub texto: String,
    pub data: String,
}

#[derive(Debug, Deserialize)]
pub struct SyncPlan {
    #[serde(default)]
    pub novos: Vec<PlannedItem>,
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

/// Termos que NUNCA podem aparecer em itens enviados ao Slack
/// (projetos pessoais, nomes internos, etc.). Definidos via env var
/// `SLACK_REDACT_TERMS` (comma-separated, case-insensitive).
pub fn redact_terms() -> Vec<String> {
    env::var("SLACK_REDACT_TERMS")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

pub fn contains_redacted(text: &str, terms: &[String]) -> bool {
    if terms.is_empty() {
        return false;
    }
    let lower = text.to_lowercase();
    terms.iter().any(|t| lower.contains(t))
}

/// Substitui ocorrências (case-insensitive) de cada termo por `[REDACTED]`.
/// Usado pra remover termos sensíveis do payload enviado ao LLM antes de mandar.
fn redact_in_text(text: &str, terms: &[String]) -> String {
    if terms.is_empty() {
        return text.to_string();
    }
    let mut out = text.to_string();
    for t in terms {
        let pat = format!("(?i){}", regex::escape(t));
        if let Ok(re) = regex::Regex::new(&pat) {
            out = re.replace_all(&out, "[REDACTED]").into_owned();
        }
    }
    out
}

fn excluded_projects() -> std::collections::HashSet<String> {
    env::var("SLACK_EXCLUDED_PROJECTS")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Verdadeiro se o próprio diretório OU qualquer ancestral tem um nome listado
/// em `SLACK_EXCLUDED_PROJECTS`. Comparação case-insensitive.
///
/// Walk dos ancestrais é o que cobre o caso "tudo dentro de uma pasta-mãe é pessoal"
/// (ex.: `~/Área de trabalho/Munique/foo` é vetado por ter `Munique` como ancestral),
/// sem precisar listar cada projeto novo individualmente.
fn is_excluded(dir: &std::path::Path) -> bool {
    let excluded = excluded_projects();
    if excluded.is_empty() {
        return false;
    }
    for ancestor in dir.ancestors() {
        if let Some(name) = ancestor.file_name().and_then(|n| n.to_str()) {
            if excluded.contains(&name.to_lowercase()) {
                return true;
            }
        }
    }
    false
}

fn minutes_by_window_in_range(
    conn: &Connection,
    from: NaiveDate,
    to: NaiveDate,
) -> rusqlite::Result<Vec<(String, i64)>> {
    let from_s = from.format("%Y-%m-%d").to_string();
    let to_s = to.format("%Y-%m-%d").to_string();
    let mut stmt = conn.prepare(
        "SELECT nome_da_janela, COUNT(*) as amostras
         FROM activity_log
         WHERE substr(timestamp, 1, 10) BETWEEN ?1 AND ?2
         GROUP BY nome_da_janela
         ORDER BY amostras DESC
         LIMIT 15",
    )?;
    let rows = stmt.query_map([&from_s, &to_s], |row| {
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

fn minutes_for_dir_in_range(conn: &Connection, dir: &Path, from: NaiveDate, to: NaiveDate) -> i64 {
    let dir_name = match dir.file_name().and_then(|n| n.to_str()) {
        Some(n) => n.to_lowercase(),
        None => return 0,
    };
    let from_s = from.format("%Y-%m-%d").to_string();
    let to_s = to.format("%Y-%m-%d").to_string();
    let pattern = format!("%{}%", dir_name);
    let mut stmt = match conn.prepare(
        "SELECT COUNT(*) FROM activity_log
         WHERE substr(timestamp, 1, 10) BETWEEN ?1 AND ?2
           AND lower(nome_da_janela) LIKE ?3",
    ) {
        Ok(s) => s,
        Err(_) => return 0,
    };
    let amostras: i64 = stmt
        .query_row([&from_s, &to_s, &pattern], |row| row.get(0))
        .unwrap_or(0);
    (amostras * POLL_INTERVAL_SECS) / 60
}

fn files_modified_in_range(root: &Path, from: NaiveDate, to: NaiveDate) -> Vec<(String, NaiveDate)> {
    let start_ts = match from
        .and_hms_opt(0, 0, 0)
        .and_then(|dt| dt.and_local_timezone(Local).single())
    {
        Some(dt) => dt.timestamp() as u64,
        None => return Vec::new(),
    };
    let end_ts = match (to + chrono::Duration::days(1))
        .and_hms_opt(0, 0, 0)
        .and_then(|dt| dt.and_local_timezone(Local).single())
    {
        Some(dt) => dt.timestamp() as u64,
        None => u64::MAX,
    };

    let mut out: Vec<(String, NaiveDate)> = Vec::new();
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
            if mtime >= start_ts && mtime < end_ts {
                if let Ok(rel) = path.strip_prefix(root) {
                    let mtime_date = chrono::DateTime::<chrono::Local>::from(
                        UNIX_EPOCH + std::time::Duration::from_secs(mtime),
                    )
                    .date_naive();
                    out.push((rel.display().to_string(), mtime_date));
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

fn build_project_activity_in_range(
    conn: &Connection,
    root: PathBuf,
    from: NaiveDate,
    to: NaiveDate,
) -> ProjectActivity {
    let root_is_git = is_git_root(&root);
    if !root_is_git {
        debug!(
            "{} não é raiz de repo git — usando só mtime",
            root.display()
        );
    }
    let git_log = if root_is_git {
        let next_day = to + chrono::Duration::days(1);
        let since = format!("--since={}T00:00:00", from);
        let until = format!("--until={}T00:00:00", next_day);
        run_git(&root, &["log", &since, &until, "--date=short", "--pretty=format:%h %ad %s", "--all"])
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
    let arquivos_modificados = files_modified_in_range(&root, from, to);
    let minutos_tela = minutes_for_dir_in_range(conn, &root, from, to);
    ProjectActivity { dir: root, minutos_tela, git_log, git_diff_stat, git_status, arquivos_modificados }
}

pub fn collect_activity_in_range(
    conn: &Connection,
    from: NaiveDate,
    to: NaiveDate,
) -> ActivitySnapshot {
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    let mut projetos: Vec<ProjectActivity> = Vec::new();

    // 1. PROJECT_ROOTS explícitas (override manual, sempre incluídas)
    for root in project_roots() {
        if !root.exists() {
            warn!("PROJECT_ROOT não existe, pulando: {}", root.display());
            continue;
        }
        if is_excluded(&root) {
            debug!("projeto excluído via SLACK_EXCLUDED_PROJECTS: {}", root.display());
            continue;
        }
        let canon = root.canonicalize().unwrap_or_else(|_| root.clone());
        seen.insert(canon);
        projetos.push(build_project_activity_in_range(conn, root, from, to));
    }

    // 2. Auto-descoberta: projetos que apareceram na tela no range
    let window_projects = top_projects_in_range(conn, from, to).unwrap_or_default();
    for p in &window_projects {
        if let Some(dir) = find_git_dir_for(&p.nome_do_projeto) {
            if is_excluded(&dir) {
                debug!("projeto excluído via SLACK_EXCLUDED_PROJECTS: {}", dir.display());
                continue;
            }
            let canon = dir.canonicalize().unwrap_or_else(|_| dir.clone());
            if seen.contains(&canon) {
                continue; // já coberto pelo PROJECT_ROOTS
            }
            seen.insert(canon);
            debug!("auto-descoberto pela tela: {}", dir.display());
            projetos.push(build_project_activity_in_range(conn, dir, from, to));
        }
    }

    let top_janelas = minutes_by_window_in_range(conn, from, to).unwrap_or_default();
    ActivitySnapshot { today: to, projetos, top_janelas }
}

fn build_todo_payload(
    snapshot: &ActivitySnapshot,
    list_items: &[ListItem],
    available_slots: usize,
) -> String {
    let mut buf = String::new();
    buf.push_str(&format!("# Data de hoje: {}\n\n", snapshot.today));

    buf.push_str("# Atividade real do período\n\n");
    for p in &snapshot.projetos {
        let nome = p
            .dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("(sem nome)");
        buf.push_str(&format!(
            "## Projeto: {} ({} min de tela)\n",
            nome, p.minutos_tela
        ));
        buf.push_str("### git log no período (formato: hash YYYY-MM-DD subject)\n");
        buf.push_str(if p.git_log.trim().is_empty() {
            "(sem commits no período)"
        } else {
            &p.git_log
        });
        buf.push_str("\n### git status (estado atual, não commitado)\n");
        buf.push_str(if p.git_status.trim().is_empty() {
            "(working tree limpo)"
        } else {
            &p.git_status
        });
        buf.push_str("\n### git diff --stat (estado atual)\n");
        buf.push_str(if p.git_diff_stat.trim().is_empty() {
            "(sem diffs)"
        } else {
            &p.git_diff_stat
        });
        buf.push_str("\n### Arquivos modificados no período (formato: YYYY-MM-DD path)\n");
        if p.arquivos_modificados.is_empty() {
            buf.push_str("(nenhum)\n");
        } else {
            for (f, d) in p.arquivos_modificados.iter().take(30) {
                buf.push_str(&format!("- {} {}\n", d, f));
            }
        }
        buf.push('\n');
    }

    buf.push_str("# Top janelas ativas hoje (sinal do que estava na tela)\n");
    // Filtra janelas que mencionam projetos vetados (`SLACK_EXCLUDED_PROJECTS`).
    // Os nomes batem case-insensitive como substring no título da janela —
    // isso evita que o LLM gere subtarefas a partir do título da janela
    // de projetos pessoais que NÃO foram coletados via build_project_activity.
    let excluded = excluded_projects();
    let mut emitted = 0;
    for (w, m) in snapshot.top_janelas.iter() {
        if emitted >= 10 {
            break;
        }
        let w_lower = w.to_lowercase();
        if excluded.iter().any(|x| w_lower.contains(x)) {
            continue;
        }
        buf.push_str(&format!("- {} min — {}\n", m, w));
        emitted += 1;
    }

    buf.push_str("\n# Lista atual da semana no Slack (NÃO duplique esses itens)\n");
    buf.push_str("Subtarefas que já existem sob a semana atual:\n");
    for it in list_items {
        let status = if it.done { "[x]" } else { "[ ]" };
        let name = it.name.as_deref().unwrap_or("(sem nome)");
        buf.push_str(&format!("- {} {} (row_id={})\n", status, name, it.row_id));
    }

    buf.push_str(&format!(
        "\n# Sua tarefa\nRetorne JSON com os trabalhos da atividade. CADA item DEVE incluir a DATA REAL (YYYY-MM-DD) em que aquele trabalho foi feito — derive da data do commit no git log ou do mtime do arquivo modificado. Se um item consolida trabalhos de múltiplos dias, use a data MAIS RECENTE entre eles. NUNCA use 'hoje' como data padrão se há informação melhor.\n\nLIMITE DURO: até {} itens novos (Slack já tem {} de 50 subtarefas no parent). Se houver mais trabalhos do que slots, NÃO descarte — CONSOLIDE itens relacionados (ex: várias atualizações de dependência → 1 item; vários ajustes de UI relacionados → 1 item). Trate cada projeto separadamente:\n  - Projetos com commits: idealmente ~1 item por commit substancial; se passar do limite, agrupe commits relacionados num item ligeiramente maior (15-20 palavras).\n  - Projetos SEM commits mas com arquivos modificados/tempo de tela: gere itens inferindo dos nomes (ex: `windscribe_configs/*.conf` → 'configurei novos servidores Windscribe'; `external_api_*.py` → 'adicionei autenticação para APIs externas'). Use a data do mtime do arquivo principal.\n  - NÃO pule projetos só porque o git log está vazio — arquivos modificados também são trabalho real.\nEm `marcar_feito` coloque qualquer row_id cuja descrição bate com algo que eu realmente fiz nesse período.\n\nFORMATO obrigatório: {{\"novos\": [{{\"texto\": \"[projeto] descrição\", \"data\": \"YYYY-MM-DD\"}}, ...], \"marcar_feito\": [\"Rec...\", ...]}}\n",
        available_slots,
        list_items.len(),
    ));
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

async fn call_gemini_async(user_payload: &str, system: &str) -> Result<String, String> {
    let key = env::var("GEMINI_API_KEY")
        .map_err(|_| "GEMINI_API_KEY não definido".to_string())?;
    let model = env::var("GEMINI_MODEL").unwrap_or_else(|_| "gemini-2.5-flash".to_string());
    let base = env::var("GEMINI_BASE_URL")
        .unwrap_or_else(|_| "https://generativelanguage.googleapis.com".to_string());

    let body = json!({
        "systemInstruction": {"parts": [{"text": system}]},
        "contents": [{"role": "user", "parts": [{"text": user_payload}]}],
        "generationConfig": {
            "temperature": 0.2,
            "responseMimeType": "application/json"
        }
    });

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| format!("erro ao construir client: {}", e))?;

    let url = format!(
        "{}/v1beta/models/{}:generateContent",
        base.trim_end_matches('/'),
        model
    );

    let resp = client
        .post(&url)
        .header("x-goog-api-key", key)
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("erro na request Gemini: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("Gemini {}: {}", status, text));
    }

    let v: Value = resp
        .json()
        .await
        .map_err(|e| format!("json inválido: {}", e))?;

    if let Some(usage) = v.get("usageMetadata") {
        debug!("gemini usage: {}", usage);
    }

    v["candidates"][0]["content"]["parts"][0]["text"]
        .as_str()
        .map(|s| s.trim().to_string())
        .ok_or_else(|| format!("resposta Gemini sem conteúdo: {}", v))
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
        .timeout(Duration::from_secs(1800))
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

async fn call_openai_async(user_payload: &str, system: &str) -> Result<String, String> {
    let api_key = env::var("OPENAI_API_KEY")
        .map_err(|_| "OPENAI_API_KEY não definido".to_string())?;
    let model = env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_string());
    let url = env::var("OPENAI_BASE_URL")
        .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());

    let body = json!({
        "model": model,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user_payload},
        ],
        "temperature": 0.2,
    });

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| format!("erro ao construir client: {}", e))?;

    let resp = client
        .post(format!("{}/chat/completions", url.trim_end_matches('/')))
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("erro na request OpenAI: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("OpenAI {}: {}", status, text));
    }

    let v: Value = resp
        .json()
        .await
        .map_err(|e| format!("json inválido: {}", e))?;
        
    v["choices"][0]["message"]["content"]
        .as_str()
        .map(|s| s.trim().to_string())
        .ok_or_else(|| format!("resposta OpenAI sem conteúdo: {}", v))
}

pub async fn generate_sync_plan(
    snapshot: &ActivitySnapshot,
    current_week_items: &[ListItem],
    available_slots: usize,
) -> Result<SyncPlan, String> {
    let raw_payload = build_todo_payload(snapshot, current_week_items, available_slots);
    let terms = redact_terms();
    let payload = redact_in_text(&raw_payload, &terms);
    debug!("payload para LLM (todo sync):\n{}", payload);
    let provider = detect_provider();
    debug!("provider selecionado: {:?}", provider);
    let raw = match provider {
        LlmProvider::Claude => call_claude_async(&payload, SYSTEM_PROMPT_TODO).await?,
        LlmProvider::Ollama => call_ollama_async(&payload, SYSTEM_PROMPT_TODO).await?,
        LlmProvider::OpenAi => call_openai_async(&payload, SYSTEM_PROMPT_TODO).await?,
        LlmProvider::Gemini => call_gemini_async(&payload, SYSTEM_PROMPT_TODO).await?,
    };
    debug!("resposta bruta do LLM:\n{}", raw);
    let mut plan = parse_sync_plan(&raw)?;
    let before = plan.novos.len();
    plan.novos.retain(|i| {
        let dirty = contains_redacted(&i.texto, &terms);
        if dirty {
            warn!("item descartado por conter termo redacted: {}", i.texto);
        }
        !dirty
    });
    if plan.novos.len() < before {
        info!(
            "filtro SLACK_REDACT_TERMS removeu {} item(ns) do plano",
            before - plan.novos.len()
        );
    }
    let existing_row_ids: std::collections::HashSet<&str> =
        current_week_items.iter().map(|i| i.row_id.as_str()).collect();
    plan.marcar_feito
        .retain(|r| existing_row_ids.contains(r.as_str()));
    Ok(plan)
}

const SYSTEM_PROMPT_CONSOLIDATE: &str = "Você é uma engenheira sênior reescrevendo uma lista de tarefas semanal pra caber no limite de 50 subtarefas do Slack. Estilo: itens curtos e atômicos quando possível, mas pode ter 15-20 palavras quando precisar consolidar 2-3 ações relacionadas num só. Mantenha o prefixo `[projeto]` de cada item, primeira pessoa, português brasileiro, tom de sênior, foco em IMPACTO. NÃO inclua coisas que não estavam na lista original. NÃO descarte trabalho — todo item original deve ter sua essência presente em algum item da lista consolidada. Retorne APENAS o JSON pedido, sem markdown ou texto extra.";

pub async fn generate_consolidation_plan(
    items: &[ListItem],
    target: usize,
) -> Result<Vec<String>, String> {
    let mut payload = String::new();
    payload.push_str(&format!(
        "# Lista atual com {} itens — consolidar para no máximo {} itens\n\n",
        items.len(),
        target
    ));
    payload.push_str("# Itens existentes (preserve a essência de TODOS)\n");
    for (i, it) in items.iter().enumerate() {
        let n = it.name.as_deref().unwrap_or("(sem nome)");
        payload.push_str(&format!("{}. {}\n", i + 1, n));
    }
    payload.push_str(&format!(
        "\n# Sua tarefa\nReescreva esta lista mantendo TODA a informação relevante mas em no máximo {} itens. CONSOLIDE itens relacionados/pequenos em itens ligeiramente maiores (15-20 palavras OK quando consolidado). Mantenha o prefixo `[projeto]` em cada item. NÃO descarte trabalho real.\n\nRetorne APENAS este JSON:\n{{\"itens\": [\"texto1\", \"texto2\", ...]}}",
        target
    ));

    debug!("payload para LLM (consolidate):\n{}", payload);
    let provider = detect_provider();
    let raw = match provider {
        LlmProvider::Claude => call_claude_async(&payload, SYSTEM_PROMPT_CONSOLIDATE).await?,
        LlmProvider::Ollama => call_ollama_async(&payload, SYSTEM_PROMPT_CONSOLIDATE).await?,
        LlmProvider::OpenAi => call_openai_async(&payload, SYSTEM_PROMPT_CONSOLIDATE).await?,
        LlmProvider::Gemini => call_gemini_async(&payload, SYSTEM_PROMPT_CONSOLIDATE).await?,
    };
    debug!("resposta bruta da LLM (consolidate):\n{}", raw);

    #[derive(Deserialize)]
    struct ConsolidateResponse {
        itens: Vec<String>,
    }
    let trimmed = raw.trim();
    let start = trimmed.find('{').ok_or("resposta sem JSON")?;
    let end = trimmed.rfind('}').ok_or("resposta sem JSON fechado")?;
    let resp: ConsolidateResponse = serde_json::from_str(&trimmed[start..=end])
        .map_err(|e| format!("parse JSON da consolidação falhou: {}", e))?;

    let mut out = resp.itens;
    if out.len() > target {
        warn!("LLM devolveu {} itens (limite {}), truncando", out.len(), target);
        out.truncate(target);
    }
    Ok(out)
}
