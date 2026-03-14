use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;

use eframe::egui;
use lore_db::{cosine_similarity, FragmentId, LoreDb, Storage};

const TOP_N: usize = 10;

// ──── Background thread messages ────

struct SearchRequest {
    /// Empty query = list all roots. Non-empty = semantic search.
    query: String,
    /// Empty = search roots globally. Non-empty = search children of these parents.
    parent_ids: Vec<FragmentId>,
}

#[derive(Clone)]
struct ResultEntry {
    id: FragmentId,
    content: String,
    depth: u32,
    score: f32,
    children_count: usize,
}

// ──── App state ────

struct ExplorerApp {
    query: String,
    results: Vec<ResultEntry>,
    depth_level: usize,
    searching: bool,
    expanded: Option<FragmentId>,

    tx: mpsc::Sender<SearchRequest>,
    rx: mpsc::Receiver<Vec<ResultEntry>>,
}

impl ExplorerApp {
    fn new(db_path: PathBuf) -> Self {
        let (query_tx, query_rx) = mpsc::channel::<SearchRequest>();
        let (result_tx, result_rx) = mpsc::channel::<Vec<ResultEntry>>();

        // Background thread owns the DB
        thread::spawn(move || {
            let storage = match Storage::open(&db_path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("Failed to open database: {}", e);
                    return;
                }
            };
            let db = LoreDb::new(storage);

            while let Ok(req) = query_rx.recv() {
                let results = if req.parent_ids.is_empty() {
                    if req.query.is_empty() {
                        // List all roots (initial load)
                        let roots = db.list_roots(None);
                        roots
                            .iter()
                            .take(TOP_N)
                            .map(|f| ResultEntry {
                                id: f.id,
                                content: f.content.clone(),
                                depth: f.depth,
                                score: f.relevance_score,
                                children_count: db.children(f.id).len(),
                            })
                            .collect()
                    } else {
                        // Semantic search on roots
                        let scored = db.query(&req.query, 0, TOP_N);
                        scored
                            .iter()
                            .map(|sf| ResultEntry {
                                id: sf.fragment.id,
                                content: sf.fragment.content.clone(),
                                depth: sf.fragment.depth,
                                score: sf.score,
                                children_count: db.children(sf.fragment.id).len(),
                            })
                            .collect()
                    }
                } else {
                    // Search children of the given parents
                    let query_embedding = db.embed_text(&req.query);
                    let mut all_results = Vec::new();

                    for parent_id in &req.parent_ids {
                        let children = db.children(*parent_id);
                        for child in &children {
                            let score = if let Some(ref qe) = query_embedding {
                                if !child.embedding.is_empty() {
                                    let sim = cosine_similarity(qe, &child.embedding);
                                    0.7 * sim + 0.3 * child.relevance_score
                                } else {
                                    child.relevance_score * 0.3
                                }
                            } else {
                                // Text fallback
                                let content_lower = child.content.to_lowercase();
                                let query_lower = req.query.to_lowercase();
                                if content_lower.contains(&query_lower) {
                                    0.8
                                } else {
                                    let words: Vec<&str> =
                                        query_lower.split_whitespace().collect();
                                    let matches = words
                                        .iter()
                                        .filter(|w| content_lower.contains(*w))
                                        .count();
                                    if matches > 0 {
                                        0.3 + 0.4 * matches as f32 / words.len() as f32
                                    } else {
                                        0.0
                                    }
                                }
                            };

                            if score > 0.0 {
                                all_results.push(ResultEntry {
                                    id: child.id,
                                    content: child.content.clone(),
                                    depth: child.depth,
                                    score,
                                    children_count: db.children(child.id).len(),
                                });
                            }
                        }
                    }

                    all_results
                        .sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
                    all_results.truncate(TOP_N);
                    all_results
                };

                let _ = result_tx.send(results);
            }
        });

        // Load roots on startup
        let _ = query_tx.send(SearchRequest {
            query: String::new(),
            parent_ids: vec![],
        });

        Self {
            query: String::new(),
            results: Vec::new(),
            depth_level: 0,
            searching: true,
            expanded: None,
            tx: query_tx,
            rx: result_rx,
        }
    }

    fn submit_search(&mut self) {
        if self.query.trim().is_empty() {
            return;
        }

        let parent_ids = if self.depth_level == 0 {
            vec![]
        } else {
            self.results.iter().take(TOP_N).map(|r| r.id).collect()
        };

        let _ = self.tx.send(SearchRequest {
            query: self.query.clone(),
            parent_ids,
        });
        self.searching = true;
        self.depth_level += 1;
    }
}

impl eframe::App for ExplorerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Poll for results
        if let Ok(results) = self.rx.try_recv() {
            self.results = results;
            self.searching = false;
            self.expanded = None;
        }

        // Request repaint while searching
        if self.searching {
            ctx.request_repaint();
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;

            // ── Search bar ──
            ui.horizontal(|ui| {
                let response = ui.add_sized(
                    [ui.available_width() - 80.0, 28.0],
                    egui::TextEdit::singleline(&mut self.query)
                        .hint_text("Search knowledge...")
                        .font(egui::TextStyle::Body),
                );

                if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    self.submit_search();
                    response.request_focus();
                }

                if ui.button("Search").clicked() {
                    self.submit_search();
                }
            });

            // ── Breadcrumb ──
            if self.depth_level > 0 {
                ui.horizontal(|ui| {
                    if ui
                        .selectable_label(false, "⟵ Roots")
                        .clicked()
                    {
                        self.depth_level = 0;
                        self.results.clear();
                        self.expanded = None;
                    }
                    for i in 1..self.depth_level {
                        ui.label("›");
                        ui.label(format!("Level {}", i));
                    }
                    if self.searching {
                        ui.spinner();
                    }
                });
                ui.separator();
            } else if self.searching {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Searching...");
                });
            }

            // ── Results ──
            if self.results.is_empty() && !self.searching && self.depth_level > 0 {
                ui.label("No results found.");
            }

            egui::ScrollArea::vertical().show(ui, |ui| {
                for entry in self.results.clone() {
                    let is_expanded = self.expanded == Some(entry.id);

                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.set_width(ui.available_width());

                        // Header line
                        ui.horizontal(|ui| {
                            let depth_label = format!("d{}", entry.depth);
                            ui.label(
                                egui::RichText::new(depth_label)
                                    .small()
                                    .color(egui::Color32::GRAY),
                            );
                            ui.label(
                                egui::RichText::new(format!("{:.2}", entry.score))
                                    .small()
                                    .color(egui::Color32::GRAY),
                            );
                            if entry.children_count > 0 {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{} children",
                                        entry.children_count
                                    ))
                                    .small()
                                    .color(egui::Color32::GRAY),
                                );
                            }
                        });

                        // Content
                        let display_text = if is_expanded {
                            entry.content.clone()
                        } else {
                            let max = 200;
                            if entry.content.len() > max {
                                let end = entry
                                    .content
                                    .char_indices()
                                    .nth(max)
                                    .map(|(i, _)| i)
                                    .unwrap_or(entry.content.len());
                                format!("{}...", &entry.content[..end])
                            } else {
                                entry.content.clone()
                            }
                        };

                        if ui
                            .add(egui::Label::new(&display_text).sense(egui::Sense::click()))
                            .clicked()
                        {
                            self.expanded = if is_expanded {
                                None
                            } else {
                                Some(entry.id)
                            };
                        }
                    });
                }
            });
        });
    }
}

fn main() -> eframe::Result {
    let db_path = std::env::var("LORE_DB_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| lore_db::lore_home().join("memory.db"));

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([700.0, 500.0])
            .with_title("Lore Explorer"),
        ..Default::default()
    };

    eframe::run_native(
        "Lore Explorer",
        options,
        Box::new(move |_cc| Ok(Box::new(ExplorerApp::new(db_path)))),
    )
}
