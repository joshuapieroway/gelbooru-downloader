//! Full-screen image viewer used by the TUI.
//!
//! The viewer draws a proper ratatui layout (header, details panel, tags panel
//! and an image frame) and then asks kitty's `icat` kitten to place the
//! full-resolution image inside the image frame.  Everything stays on the same
//! alternate screen, so returning to the search results is instant and the
//! search page/scroll position/query are preserved.

use crate::api::client::GelbooruClient;
use crate::api::models::Post;
use crate::config::Config;
use crate::database::history::HistoryDb;
use crate::downloader::engine::{
    DownloadEngine, DownloadEntry, DownloadOutcome, DownloadQueue, DownloadStatus,
};
use crate::utils::paths;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind};
use futures::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;

type TuiTerminal = Terminal<CrosstermBackend<io::Stdout>>;

const BORDER: Color = Color::Rgb(58, 70, 92);
const LABEL: Color = Color::Rgb(112, 124, 146);
const VALUE: Color = Color::Rgb(226, 232, 242);
const MUTED: Color = Color::Rgb(140, 150, 168);
const TAG: Color = Color::Rgb(184, 196, 212);
const MAX_PREVIEW_BYTES: u64 = 128 * 1024 * 1024;
const ICAT_TIMEOUT: Duration = Duration::from_secs(20);

/// How many previews may download in the background at once.
const PREFETCH_CONCURRENCY: usize = 3;
/// Upper bound on queued/finished prefetch handles kept around.
const MAX_PREFETCH_TASKS: usize = 6;

type PreviewTask = JoinHandle<Result<(PathBuf, bool), String>>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum MediaKind {
    Image,
    Gif,
    Video,
    Other,
}

enum Phase {
    Loading,
    Ready,
    Error(String),
    Unsupported(String),
}

/// Everything the viewer needs from the search tab.
pub struct ViewerRequest {
    pub posts: Vec<Post>,
    pub start: usize,
    pub db: Option<Arc<HistoryDb>>,
    pub client: Option<GelbooruClient>,
    pub query: String,
    pub per_page: usize,
    pub page: usize,
    pub total: i64,
}

/// Show the viewer starting at `start` in `posts`.
///
/// The caller keeps its own search state untouched; this function only draws on
/// the alternate screen and cleans the images up before returning.  The viewer
/// allows navigation past the bounds of `posts` to fetch adjacent pages from the
/// API, enabling indefinite browsing through the search results.
/// Pressing `d` queues downloads that show progress in the status area.
pub async fn show(terminal: &mut TuiTerminal, cfg: &Config, request: ViewerRequest) {
    let ViewerRequest {
        posts,
        start,
        db,
        client,
        query,
        per_page: search_per_page,
        page: search_page,
        total: search_total,
    } = request;
    if posts.is_empty() {
        return;
    }

    let mut index = start.min(posts.len() - 1);
    let mut all_posts: Vec<Post> = posts;
    // The caller hands us exactly one page of results (the one the search tab
    // is showing), so the loaded range starts out as a single page.  Pressing
    // left/right past the ends fetches adjacent pages, in either direction.
    let mut first_page = search_page.max(1);
    let mut last_page = search_page.max(1);
    let mut at_end = false;
    let quick_dir = quick_download_dir(cfg);

    // Shared download queue for showing multiple pending/ongoing downloads.
    let queue = Arc::new(DownloadQueue::new());
    // One engine for every viewer download: clones share the HTTP client, the
    // semaphore and the queue, so concurrent downloads respect
    // `downloads.concurrency` instead of each starting its own engine.
    let engine = DownloadEngine::with_queue(cfg.clone(), db.clone(), Some(queue.clone()));

    // Track active download handles so we can check completion
    let mut active_downloads: Vec<tokio::task::JoinHandle<DownloadOutcome>> = Vec::new();

    // Refreshed while the viewer is idle so multiple downloads stay visible.
    let mut downloads_summary: String;
    // Result of the most recently finished quick download, shown next to the
    // live queue so a brief "Saved ..." message is not lost.
    let mut download_notice = String::new();

    // Previews for the neighbouring posts are fetched in the background so
    // stepping through the gallery rarely has to wait on the network.
    let prefetch_permits = Arc::new(Semaphore::new(PREFETCH_CONCURRENCY));
    let mut prefetched: HashMap<i64, PreviewTask> = HashMap::new();

    terminal.clear().ok();

    'gallery: loop {
        let post = &all_posts[index];
        let mut tags_scroll: u16 = 0;
        let mut phase;
        #[allow(unused_assignments)]
        let mut status = String::new();
        let mut preview: Option<PathBuf> = None;

        // Collect finished quick downloads. The queue itself is updated by the
        // engine; the handles only provide the final outcome for the status
        // line (saved path, duplicate, or error).
        for outcome in reap_downloads(&mut active_downloads).await {
            download_notice = quick_download_status(&outcome);
        }

        // Get current download queue entries for display. The footer refreshes
        // this while the viewer is idle, so several downloads stay visible.
        let queue_entries = queue.entries().await;
        downloads_summary =
            combine_download_status(&download_notice, &visible_download_entries(&queue_entries));

        let kind = media_kind(post);
        let cached_path = preview_path(post);
        // Only static images are previewed. Videos and animated GIFs get a
        // clear explanation instead of being handed to an external decoder.
        let previewable = is_previewable(kind);
        let cached = previewable && preview_is_cached(&cached_path);

        if !previewable {
            // Videos and unknown file types are never handed to kitty: show a
            // friendly explanation instead of letting a decoder fail/crash.
            clear_images();
            phase = Phase::Unsupported(match kind {
                MediaKind::Video => "Video files cannot be previewed in the terminal.".to_string(),
                _ => "This file type cannot be previewed in the terminal.".to_string(),
            });
            status = "Not previewable - press o to open the post on Gelbooru".to_string();
        } else if cached {
            clear_images();
            phase = Phase::Ready;
            status = ready_status(post, &cached_path);
            preview = Some(cached_path.clone());
        } else {
            phase = Phase::Loading;
            let is_sample = should_use_sample(post);
            let quality_warning = is_quality_warning_needed(post);
            status = if is_sample {
                if quality_warning {
                    "Downloading preview (sample - press d for full-resolution)...".to_string()
                } else {
                    "Downloading preview (sample)...".to_string()
                }
            } else {
                "Downloading full-resolution preview...".to_string()
            };

            // Show the loading card before the (possibly slow) download.
            let _ = draw(
                terminal,
                &all_posts,
                index,
                &phase,
                tags_scroll,
                &status,
                &downloads_summary,
            );
            clear_images();

            match load_preview(post, prefetched.remove(&post.id)).await {
                Ok((path, _is_sample)) => {
                    preview = Some(path.clone());
                    phase = Phase::Ready;
                    status = ready_status(post, &path);
                }
                Err(e) => {
                    phase = Phase::Error(e);
                    status = "Could not download preview".to_string();
                }
            }
        }

        // Draw the final state for this post, then place the image.  Keeping
        // drawing and placement in this order means the image never covers the
        // loading/error panel and ratatui can cleanly erase old messages.
        let (mut last_img_rect, mut max_tags_scroll) = draw(
            terminal,
            &all_posts,
            index,
            &phase,
            tags_scroll,
            &status,
            &downloads_summary,
        );

        if let (Phase::Ready, Some(path)) = (&phase, &preview) {
            if let Err(e) = place_image(post, path, last_img_rect) {
                clear_images();
                phase = Phase::Error(e);
                status = "Preview could not be displayed".to_string();
                let (_, m) = draw(
                    terminal,
                    &all_posts,
                    index,
                    &phase,
                    tags_scroll,
                    &status,
                    &downloads_summary,
                );
                max_tags_scroll = m;
            }
        }

        // Warm the adjacent previews while the user studies this image.
        // Finished handles are pruned first so slots stay available.
        prefetched.retain(|_, handle| !handle.is_finished());
        if index > 0 {
            spawn_prefetch(&all_posts[index - 1], &mut prefetched, &prefetch_permits);
        }
        if index + 1 < all_posts.len() {
            spawn_prefetch(&all_posts[index + 1], &mut prefetched, &prefetch_permits);
        }

        loop {
            let ev = match event::poll(Duration::from_millis(100)) {
                Ok(true) => match event::read() {
                    Ok(ev) => Some(ev),
                    Err(_) => {
                        clear_images();
                        terminal.clear().ok();
                        return;
                    }
                },
                Ok(false) => None,
                Err(_) => {
                    clear_images();
                    terminal.clear().ok();
                    return;
                }
            };

            let Some(ev) = ev else {
                // Refresh download progress while the viewer is idle so
                // several active downloads remain visible.
                let mut changed = false;
                for outcome in reap_downloads(&mut active_downloads).await {
                    download_notice = quick_download_status(&outcome);
                    // The queued/active message has served its purpose; the
                    // finished notice now carries the result for this post.
                    clear_download_status(&mut status, outcome.post_id);
                    changed = true;
                }
                let entries = queue.entries().await;
                let summary =
                    combine_download_status(&download_notice, &visible_download_entries(&entries));
                if summary != downloads_summary {
                    downloads_summary = summary;
                    changed = true;
                }
                if changed {
                    let _ = draw(
                        terminal,
                        &all_posts,
                        index,
                        &phase,
                        tags_scroll,
                        &status,
                        &downloads_summary,
                    );
                }
                continue;
            };

            match ev {
                Event::Key(k) if k.kind == KeyEventKind::Press => {
                    if k.modifiers.contains(KeyModifiers::CONTROL)
                        && matches!(k.code, KeyCode::Char('c'))
                    {
                        clear_images();
                        terminal.clear().ok();
                        return;
                    }
                    match k.code {
                        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter => {
                            clear_images();
                            terminal.clear().ok();
                            return;
                        }
                        KeyCode::Left | KeyCode::Char('h') => {
                            if index > 0 {
                                index -= 1;
                                continue 'gallery;
                            }
                            // Already at the first post - fetch the previous
                            // page so browsing is not limited to one page.
                            if first_page > 1 {
                                if let Some(client) = &client {
                                    let prev = first_page - 1;
                                    status = format!("Loading page {}...", prev);
                                    let _ = draw(
                                        terminal,
                                        &all_posts,
                                        index,
                                        &phase,
                                        tags_scroll,
                                        &status,
                                        &downloads_summary,
                                    );

                                    match load_page(client, &query, search_per_page, prev).await {
                                        Ok(new_posts) => {
                                            if new_posts.is_empty() {
                                                status = "No previous page".to_string();
                                            } else {
                                                // Prepend the previous page's
                                                // posts and step one to the
                                                // left: onto its last post.
                                                let prev_len = new_posts.len();
                                                let mut full_posts = new_posts;
                                                full_posts.append(&mut all_posts);
                                                all_posts = full_posts;
                                                first_page = prev;
                                                index = prev_len - 1;
                                                tags_scroll = 0;
                                                status = loaded_status(
                                                    search_total,
                                                    first_page,
                                                    all_posts.len(),
                                                );
                                            }
                                        }
                                        Err(e) => {
                                            status = format!("Couldn't load previous page: {e}")
                                        }
                                    }
                                    let _ = draw(
                                        terminal,
                                        &all_posts,
                                        index,
                                        &phase,
                                        tags_scroll,
                                        &status,
                                        &downloads_summary,
                                    );
                                    continue 'gallery;
                                } else {
                                    status = "Beginning of results".to_string();
                                    let _ = draw(
                                        terminal,
                                        &all_posts,
                                        index,
                                        &phase,
                                        tags_scroll,
                                        &status,
                                        &downloads_summary,
                                    );
                                }
                            }
                        }
                        KeyCode::Right
                        | KeyCode::Char('l')
                        | KeyCode::Char('n')
                        | KeyCode::Char(' ') => {
                            if index + 1 < all_posts.len() {
                                index += 1;
                                continue 'gallery;
                            }
                            // At the last post - fetch the next page unless we
                            // already know there is nothing beyond it.
                            if !at_end {
                                if let Some(client) = &client {
                                    let next = last_page + 1;
                                    status = format!("Loading page {}...", next);
                                    let _ = draw(
                                        terminal,
                                        &all_posts,
                                        index,
                                        &phase,
                                        tags_scroll,
                                        &status,
                                        &downloads_summary,
                                    );

                                    match load_page(client, &query, search_per_page, next).await {
                                        Ok(new_posts) => {
                                            if new_posts.is_empty() {
                                                at_end = true;
                                                status = end_status(search_total, all_posts.len());
                                            } else {
                                                // Append the next page's posts
                                                // and step one to the right:
                                                // onto its first post.
                                                let new_index = all_posts.len();
                                                all_posts.extend(new_posts);
                                                last_page = next;
                                                index = new_index;
                                                tags_scroll = 0;
                                                status = loaded_status(
                                                    search_total,
                                                    last_page,
                                                    all_posts.len(),
                                                );
                                            }
                                        }
                                        Err(e) => status = format!("Couldn't load more posts: {e}"),
                                    }
                                    let _ = draw(
                                        terminal,
                                        &all_posts,
                                        index,
                                        &phase,
                                        tags_scroll,
                                        &status,
                                        &downloads_summary,
                                    );
                                    continue 'gallery;
                                } else {
                                    status = end_status(search_total, all_posts.len());
                                    let _ = draw(
                                        terminal,
                                        &all_posts,
                                        index,
                                        &phase,
                                        tags_scroll,
                                        &status,
                                        &downloads_summary,
                                    );
                                }
                            } else {
                                status = end_status(search_total, all_posts.len());
                                let _ = draw(
                                    terminal,
                                    &all_posts,
                                    index,
                                    &phase,
                                    tags_scroll,
                                    &status,
                                    &downloads_summary,
                                );
                            }
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            let new = tags_scroll.saturating_sub(1);
                            if new != tags_scroll || max_tags_scroll > 0 {
                                tags_scroll = new;
                                let _ = draw(
                                    terminal,
                                    &all_posts,
                                    index,
                                    &phase,
                                    tags_scroll,
                                    &status,
                                    &downloads_summary,
                                );
                            }
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            let new = (tags_scroll + 1).min(max_tags_scroll);
                            tags_scroll = new;
                            let _ = draw(
                                terminal,
                                &all_posts,
                                index,
                                &phase,
                                tags_scroll,
                                &status,
                                &downloads_summary,
                            );
                        }
                        KeyCode::PageUp => {
                            tags_scroll = tags_scroll.saturating_sub(6);
                            let _ = draw(
                                terminal,
                                &all_posts,
                                index,
                                &phase,
                                tags_scroll,
                                &status,
                                &downloads_summary,
                            );
                        }
                        KeyCode::PageDown => {
                            tags_scroll = (tags_scroll + 6).min(max_tags_scroll);
                            let _ = draw(
                                terminal,
                                &all_posts,
                                index,
                                &phase,
                                tags_scroll,
                                &status,
                                &downloads_summary,
                            );
                        }
                        KeyCode::Home => {
                            tags_scroll = 0;
                            let _ = draw(
                                terminal,
                                &all_posts,
                                index,
                                &phase,
                                tags_scroll,
                                &status,
                                &downloads_summary,
                            );
                        }
                        KeyCode::End => {
                            tags_scroll = max_tags_scroll;
                            let _ = draw(
                                terminal,
                                &all_posts,
                                index,
                                &phase,
                                tags_scroll,
                                &status,
                                &downloads_summary,
                            );
                        }
                        KeyCode::Char('c') => {
                            if copy_to_clipboard(&post.id.to_string()) {
                                status = format!("Copied post ID {}", post.id);
                            } else {
                                status =
                                    "Clipboard unavailable (install wl-copy or xclip)".to_string();
                            }
                            let _ = draw(
                                terminal,
                                &all_posts,
                                index,
                                &phase,
                                tags_scroll,
                                &status,
                                &downloads_summary,
                            );
                        }
                        KeyCode::Char('t') => {
                            if copy_to_clipboard(&post.tags) {
                                status = format!("Copied {} tags", post.tag_list().len());
                            } else {
                                status =
                                    "Clipboard unavailable (install wl-copy or xclip)".to_string();
                            }
                            let _ = draw(
                                terminal,
                                &all_posts,
                                index,
                                &phase,
                                tags_scroll,
                                &status,
                                &downloads_summary,
                            );
                        }
                        KeyCode::Char('o') => {
                            match open_post(post) {
                                Ok(()) => {
                                    status = format!("Opened post #{} on Gelbooru", post.id);
                                }
                                Err(e) => status = e,
                            }
                            let _ = draw(
                                terminal,
                                &all_posts,
                                index,
                                &phase,
                                tags_scroll,
                                &status,
                                &downloads_summary,
                            );
                        }
                        KeyCode::Char('d') => {
                            if queue.is_active(post.id).await {
                                status = format!("Post #{} is already downloading", post.id);
                            } else {
                                // Queue the download; clones of the shared
                                // engine enforce the configured concurrency
                                // limit across every queued post.
                                queue.add_queued(post.id).await;
                                let entries = queue.entries().await;
                                downloads_summary = combine_download_status(
                                    &download_notice,
                                    &visible_download_entries(&entries),
                                );

                                status = format!("Queued download for post #{}...", post.id);

                                let target = quick_dir.clone();
                                let download_post = post.clone();
                                let worker = engine.clone();
                                let handle = tokio::spawn(async move {
                                    worker.download_post(download_post, Some(target)).await
                                });

                                // Track the active download
                                active_downloads.push(handle);
                            }
                            let _ = draw(
                                terminal,
                                &all_posts,
                                index,
                                &phase,
                                tags_scroll,
                                &status,
                                &downloads_summary,
                            );
                        }
                        _ => {}
                    }
                }
                Event::Mouse(m) => {
                    let new = match m.kind {
                        MouseEventKind::ScrollUp => tags_scroll.saturating_sub(3),
                        MouseEventKind::ScrollDown => (tags_scroll + 3).min(max_tags_scroll),
                        _ => tags_scroll,
                    };
                    if new != tags_scroll {
                        tags_scroll = new;
                        let _ = draw(
                            terminal,
                            &all_posts,
                            index,
                            &phase,
                            tags_scroll,
                            &status,
                            &downloads_summary,
                        );
                    }
                }
                Event::Resize(_, _) => {
                    clear_images();
                    let out = draw(
                        terminal,
                        &all_posts,
                        index,
                        &phase,
                        tags_scroll,
                        &status,
                        &downloads_summary,
                    );
                    last_img_rect = out.0;
                    max_tags_scroll = out.1;
                    if let (Phase::Ready, Some(path)) = (&phase, &preview) {
                        let _ = place_image(post, path, last_img_rect);
                    }
                }
                _ => {}
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Layout / drawing
// ---------------------------------------------------------------------------

struct Regions {
    header: Rect,
    status: Rect,
    hints: Rect,
    details: Rect,
    tags: Rect,
    image: Rect,
}

fn regions(area: Rect) -> Regions {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(7),
            Constraint::Length(2),
        ])
        .split(area);

    let main = rows[1];
    let (details, tags, image) = if main.width >= 78 {
        // Wide terminal: details/tags on the left, image on the right.
        let info_w = ((main.width as u32 * 38 / 100) as u16).clamp(34, 48);
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(info_w),
                Constraint::Length(1),
                Constraint::Min(36),
            ])
            .split(main);
        let info = cols[0];
        let detail_h = 11.min(info.height.saturating_sub(3)).max(3);
        let info_rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(detail_h), Constraint::Min(3)])
            .split(info);
        (info_rows[0], info_rows[1], cols[2])
    } else {
        // Narrow terminal: image on top, details/tags underneath.
        let img_h = ((main.height as u32 * 52 / 100) as u16)
            .max(7)
            .min(main.height.saturating_sub(6).max(7));
        let split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(img_h),
                Constraint::Length(1),
                Constraint::Min(6),
            ])
            .split(main);
        let info = split[2];
        let detail_h = 10.min(info.height.saturating_sub(2)).max(3);
        let info_rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(detail_h), Constraint::Min(2)])
            .split(info);
        (info_rows[0], info_rows[1], split[0])
    };

    Regions {
        header: rows[0],
        status: Rect::new(rows[2].x, rows[2].y, rows[2].width, 1),
        hints: Rect::new(rows[2].x, rows[2].y + 1, rows[2].width, 1),
        details,
        tags,
        image,
    }
}

/// Draw one viewer frame. Returns the image interior rect and the max tag
/// scroll offset so key handling can clamp properly.
fn draw(
    terminal: &mut TuiTerminal,
    posts: &[Post],
    index: usize,
    phase: &Phase,
    tags_scroll: u16,
    status: &str,
    downloads: &str,
) -> (Rect, u16) {
    let mut img_rect = Rect::default();
    let mut max_scroll = 0u16;
    let post = &posts[index];

    let _ = terminal.draw(|f| {
        let area = f.area();
        let reg = regions(area);

        draw_header(f, post, index, posts.len(), reg.header);
        let out = draw_image_block(f, post, phase, reg.image);
        img_rect = out.0;
        let tag_out = draw_details_and_tags(f, post, reg.details, reg.tags, tags_scroll);
        max_scroll = tag_out;
        draw_footer(f, phase, status, downloads, reg.status, reg.hints);
    });

    (img_rect, max_scroll)
}

fn draw_header(f: &mut ratatui::Frame, post: &Post, index: usize, total: usize, area: Rect) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(24), Constraint::Length(44)])
        .split(area);

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("▌", Style::default().fg(Color::Cyan)),
            Span::styled(
                " GELBOORU VIEWER",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
        ])),
        cols[0],
    );

    let rating = rating_color(&post.rating);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("● ", Style::default().fg(rating)),
            Span::styled(
                format!("{:<11}", post.rating),
                Style::default().fg(rating).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("ID {}  ·  {} / {}", post.id, index + 1, total),
                Style::default().fg(MUTED),
            ),
        ]))
        .alignment(Alignment::Right),
        cols[1],
    );
}

fn draw_image_block(f: &mut ratatui::Frame, post: &Post, phase: &Phase, area: Rect) -> (Rect, u16) {
    let border = rating_border(&post.rating);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border))
        .title(Line::from(vec![
            Span::styled(
                " Preview ",
                Style::default()
                    .fg(rating_color(&post.rating))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("· {} ", display_ext(post)),
                Style::default().fg(Color::DarkGray),
            ),
        ]));

    let inner = block.inner(area);
    f.render_widget(block, area);

    match phase {
        Phase::Ready => {}
        Phase::Loading => {
            centered_message(
                f,
                inner,
                vec![
                    Line::from(Span::styled(
                        "Loading preview...",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    )),
                    Line::from(""),
                    Line::from(Span::styled(
                        "Fetching the full-resolution image",
                        Style::default().fg(MUTED),
                    )),
                ],
            );
        }
        Phase::Unsupported(reason) => {
            let width = inner.width.saturating_sub(10).max(16) as usize;
            let mut lines = vec![
                Line::from(Span::styled(
                    "Preview not available",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
            ];
            for chunk in wrap_text(reason, width).into_iter().take(3) {
                lines.push(Line::from(Span::styled(chunk, Style::default().fg(MUTED))));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Press o to open this post on Gelbooru",
                Style::default().fg(MUTED),
            )));
            centered_message(f, inner, lines);
        }
        Phase::Error(err) => {
            let width = inner.width.saturating_sub(10).max(16) as usize;
            let mut lines = vec![
                Line::from(Span::styled(
                    "Preview unavailable",
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
            ];
            for chunk in wrap_text(err, width).into_iter().take(4) {
                lines.push(Line::from(Span::styled(chunk, Style::default().fg(MUTED))));
            }
            centered_message(f, inner, lines);
        }
    }

    (inner, 0)
}

fn draw_details_and_tags(
    f: &mut ratatui::Frame,
    post: &Post,
    details_area: Rect,
    tags_area: Rect,
    tags_scroll: u16,
) -> u16 {
    // Details
    let details_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(BORDER))
        .title(Span::styled(
            " Details ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
    let details_inner = details_block.inner(details_area);
    f.render_widget(details_block, details_area);
    if details_inner.width > 0 && details_inner.height > 0 {
        f.render_widget(
            Paragraph::new(detail_lines(post, details_inner.width)),
            details_inner,
        );
    }

    // Tags
    let tag_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(BORDER))
        .title(Line::from(vec![
            Span::styled(
                " Tags ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("· {}  ", post.tag_list().len()),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
    let tags_inner = tag_block.inner(tags_area);
    f.render_widget(tag_block, tags_area);

    let lines = tag_lines(&post.tags, tags_inner.width.max(8) as usize);
    let visible = tags_inner.height.max(1) as usize;
    let max_scroll = lines.len().saturating_sub(visible).min(u16::MAX as usize) as u16;
    let scroll = tags_scroll.min(max_scroll);

    if tags_inner.width > 0 && tags_inner.height > 0 {
        f.render_widget(Paragraph::new(lines).scroll((scroll, 0)), tags_inner);
    }

    max_scroll
}

fn draw_footer(
    f: &mut ratatui::Frame,
    phase: &Phase,
    status: &str,
    downloads: &str,
    status_area: Rect,
    hints_area: Rect,
) {
    let (color, fallback) = match phase {
        Phase::Loading => (Color::Yellow, "Downloading full-resolution preview..."),
        Phase::Ready => (Color::Green, "Ready"),
        Phase::Error(_) => (Color::Red, "Preview unavailable"),
        Phase::Unsupported(_) => (Color::Yellow, "Media preview not supported"),
    };
    let text = match (status.trim().is_empty(), downloads.trim().is_empty()) {
        (true, true) => fallback.to_string(),
        (true, false) => downloads.to_string(),
        (false, true) => status.to_string(),
        (false, false) => format!("{status}  ·  {downloads}"),
    };

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" ● ", Style::default().fg(color)),
            Span::styled(
                text,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
        ])),
        status_area,
    );

    let key = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let hint = Style::default().fg(MUTED);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(" "),
            Span::styled("←/→", key),
            Span::styled(" nav   ", hint),
            Span::styled("j/k", key),
            Span::styled(" tags   ", hint),
            Span::styled("c", key),
            Span::styled(" ID   ", hint),
            Span::styled("t", key),
            Span::styled(" tags   ", hint),
            Span::styled("d", key),
            Span::styled(" download   ", hint),
            Span::styled("o", key),
            Span::styled(" post   ", hint),
            Span::styled("Esc", key),
            Span::styled(" close", hint),
        ])),
        hints_area,
    );
}

fn centered_message(f: &mut ratatui::Frame, area: Rect, lines: Vec<Line<'static>>) {
    if area.width < 4 || area.height < 1 || lines.is_empty() {
        return;
    }
    let width = area.width.saturating_sub(4).clamp(1, 66);
    let height = (lines.len() as u16).min(area.height);
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    let rect = Rect {
        x,
        y,
        width,
        height,
    };
    f.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        rect,
    );
}

// ---------------------------------------------------------------------------
// Details / tags
// ---------------------------------------------------------------------------

fn detail_lines(post: &Post, width: u16) -> Vec<Line<'static>> {
    let avail = width.saturating_sub(11).max(1) as usize;

    let artists: Vec<String> = post
        .tag_list()
        .into_iter()
        .filter(|t| t.starts_with("artist:"))
        .map(|t| t.trim_start_matches("artist:").to_string())
        .collect();
    let artist = if artists.is_empty() {
        "-".to_string()
    } else {
        artists.join(", ")
    };

    let source = if post.source.trim().is_empty() {
        "-".to_string()
    } else {
        post.source.clone()
    };
    let created = if post.created_at.trim().is_empty() {
        "-".to_string()
    } else {
        post.created_at.clone()
    };

    let size = preview_size(post);
    let size_text = if size > 0 {
        human_bytes(size)
    } else {
        "unknown".to_string()
    };
    let format = format!("{} · {}", display_ext(post), size_text);

    let mut lines = vec![
        row("ID", post.id.to_string(), VALUE),
        row(
            "Rating",
            format!("● {}", post.rating),
            rating_color(&post.rating),
        ),
        row(
            "Dimensions",
            format!("{} × {} px", post.width, post.height),
            VALUE,
        ),
        row("Score", post.score.to_string(), VALUE),
        row("Format", format, VALUE),
        row(
            "Artist",
            truncate_chars(&artist, avail),
            if artists.is_empty() { MUTED } else { VALUE },
        ),
        row("Created", truncate_chars(&created, avail), MUTED),
    ];

    // Split a long source across at most two lines with a hanging indent, so
    // URLs don't turn into an unreadable clipped field.
    let source_color = if post.source.trim().is_empty() {
        MUTED
    } else {
        Color::Rgb(126, 174, 204)
    };
    let chars: Vec<char> = source.chars().collect();
    let first: String = chars.iter().take(avail).collect();
    lines.push(row("Source", first, source_color));
    if chars.len() > avail {
        let mut second: String = chars.iter().skip(avail).take(avail).collect();
        if chars.len() > avail * 2 {
            second.pop();
            second.push('…');
        }
        lines.push(Line::from(vec![
            Span::raw(" ".repeat(11)),
            Span::styled(second, Style::default().fg(source_color)),
        ]));
    }

    lines
}

fn row(label: &str, value: String, color: Color) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<11}"), Style::default().fg(LABEL)),
        Span::styled(value, Style::default().fg(color)),
    ])
}

fn tag_lines(tags: &str, width: usize) -> Vec<Line<'static>> {
    let width = width.max(6);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut line_len = 0usize;

    for tag in tags.split_whitespace() {
        let tlen = tag.chars().count();
        let style = Style::default().fg(tag_color(tag));

        // Tags longer than the panel are hard-wrapped instead of truncated, so
        // every tag remains fully readable (the tag panel scrolls).
        if tlen > width {
            if !spans.is_empty() {
                lines.push(Line::from(std::mem::take(&mut spans)));
                line_len = 0;
            }
            let chars: Vec<char> = tag.chars().collect();
            for chunk in chars.chunks(width) {
                lines.push(Line::from(Span::styled(
                    chunk.iter().collect::<String>(),
                    style,
                )));
            }
            continue;
        }

        if !spans.is_empty() && line_len + 1 + tlen > width {
            lines.push(Line::from(std::mem::take(&mut spans)));
            line_len = 0;
        }
        if !spans.is_empty() {
            spans.push(Span::raw(" "));
            line_len += 1;
        }
        spans.push(Span::styled(tag.to_string(), style));
        line_len += tlen;
    }

    if !spans.is_empty() {
        lines.push(Line::from(spans));
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "no tags",
            Style::default().fg(Color::DarkGray),
        )));
    }
    lines
}

fn tag_color(tag: &str) -> Color {
    if tag.starts_with("artist:") {
        Color::Rgb(226, 142, 200)
    } else if tag.starts_with("copyright:") {
        Color::Rgb(228, 198, 122)
    } else if tag.starts_with("character:") {
        Color::Rgb(132, 210, 162)
    } else if tag.starts_with("meta:") {
        Color::Rgb(228, 164, 122)
    } else if tag == "video" || tag == "animated" || tag == "webm" || tag == "mp4" {
        Color::Rgb(150, 190, 230)
    } else {
        TAG
    }
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let width = width.max(4);
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();

    let flush = |current: &mut String, lines: &mut Vec<String>| {
        if !current.is_empty() {
            lines.push(std::mem::take(current));
        }
    };

    for word in text.split_whitespace() {
        let word: Vec<char> = word.chars().collect();
        let wlen = word.len();

        if current.is_empty() {
            if wlen <= width {
                current.extend(word);
            } else {
                for chunk in word.chunks(width) {
                    let s: String = chunk.iter().collect();
                    lines.push(s);
                }
                // The last chunk may be a full line; keep buffer empty.
            }
        } else if current.chars().count() + 1 + wlen <= width {
            current.push(' ');
            current.extend(word);
        } else {
            flush(&mut current, &mut lines);
            if wlen <= width {
                current.extend(word);
            } else {
                for chunk in word.chunks(width) {
                    let s: String = chunk.iter().collect();
                    lines.push(s);
                }
            }
        }
    }
    flush(&mut current, &mut lines);

    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

// ---------------------------------------------------------------------------
// Images / media helpers
// ---------------------------------------------------------------------------

fn media_kind(post: &Post) -> MediaKind {
    match post.extension().as_str() {
        "gif" => MediaKind::Gif,
        "mp4" | "webm" | "mkv" | "mov" | "avi" | "wmv" | "flv" | "m4v" | "mpg" | "mpeg" | "ogv"
        | "ts" => MediaKind::Video,
        "jpg" | "jpeg" | "png" | "webp" | "bmp" | "avif" | "jxl" | "tif" | "tiff" => {
            MediaKind::Image
        }
        _ => MediaKind::Other,
    }
}

fn is_previewable(kind: MediaKind) -> bool {
    matches!(kind, MediaKind::Image | MediaKind::Gif)
}

/// Determine which URL to use for preview.
///
/// Gelbooru provides:
/// - preview_url: ~200px thumbnail (INSTANT loading)
/// - sample_url: ~1000px max dimension (FAST loading)
/// - file_url: full resolution
///
/// FAST strategy with 60% minimum quality:
/// - Images <= 1667px: sample would be >= 60%, use full for best quality
/// - Images > 1667px: sample would be < 60%, use sample for FAST loading
///   and show warning that 'd' key downloads full resolution
fn should_use_sample(post: &Post) -> bool {
    let max_dim = post.width.max(post.height);

    // Use full resolution for small images - download is fast, quality is good
    if max_dim <= 1667 {
        return false;
    }

    // Use sample for larger images - ensures FAST loading even for huge files
    // Sample at 1000px for a 3000px image = 33% - but still FAST
    // User can press 'd' to get full resolution
    true
}

fn is_quality_warning_needed(post: &Post) -> bool {
    let max_dim = post.width.max(post.height);
    max_dim > 1667
}

fn display_ext(post: &Post) -> String {
    let ext = post.extension();
    if ext.is_empty() {
        "unknown".to_string()
    } else {
        ext.to_uppercase()
    }
}

fn preview_path(post: &Post) -> PathBuf {
    let ext = post.extension();
    let ext = if ext.is_empty() {
        "img".to_string()
    } else {
        ext
    };
    paths::data_dir()
        .join("previews")
        .join(format!("{}_preview.{}", post.id, ext))
}

fn preview_size(post: &Post) -> u64 {
    if let Ok(meta) = std::fs::metadata(preview_path(post)) {
        if meta.is_file() && meta.len() > 0 {
            return meta.len();
        }
    }
    post.file_size.max(0) as u64
}

/// A cached preview is any non-empty regular file.  Downloads are written to a
/// unique temporary file and atomically renamed, so readers never observe a
/// partially-written preview.
fn preview_is_cached(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.len() > 0)
        .unwrap_or(false)
}

/// Unique temporary path next to `target`, keeping the final rename atomic and
/// allowing a background prefetch and an on-demand fetch to coexist safely.
fn preview_tmp_path(target: &Path) -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let mut name = target
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_else(|| "preview".into());
    name.push(format!(".{}.{seq}.part", std::process::id()));
    target.with_file_name(name)
}

/// Shared HTTP client for previews.  Reusing it keeps TLS sessions and keep-
/// alive connections warm, which is a large win when browsing image by image.
fn preview_client() -> Result<&'static reqwest::Client, String> {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    if let Some(client) = CLIENT.get() {
        return Ok(client);
    }
    // These settings are fixed, so the builder cannot realistically fail; keep
    // the error path anyway so the viewer reports it instead of panicking.
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(45))
        .user_agent("gelbooru-downloader/0.1")
        .redirect(reqwest::redirect::Policy::none())
        .pool_max_idle_per_host(4)
        .tcp_keepalive(Some(Duration::from_secs(10)))
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;
    let _ = CLIENT.set(client);
    CLIENT
        .get()
        .ok_or_else(|| "HTTP client unavailable".to_string())
}

/// Resolve a preview, waiting for an in-flight prefetch when one exists instead
/// of starting a duplicate request.
async fn load_preview(
    post: &Post,
    pending: Option<PreviewTask>,
) -> Result<(PathBuf, bool), String> {
    if let Some(task) = pending {
        match task.await {
            Ok(Ok(path)) => return Ok(path),
            Ok(Err(e)) => tracing::debug!("prefetch for post {} failed: {e}", post.id),
            Err(e) => tracing::debug!("prefetch task for post {} failed: {e}", post.id),
        }
    }
    fetch_preview(post).await
}

/// Spawn a bounded background download for `post` when it is previewable and
/// not cached yet.
fn spawn_prefetch(post: &Post, pending: &mut HashMap<i64, PreviewTask>, permits: &Arc<Semaphore>) {
    if !is_previewable(media_kind(post))
        || preview_is_cached(&preview_path(post))
        || pending.contains_key(&post.id)
        || pending.len() >= MAX_PREFETCH_TASKS
    {
        return;
    }

    let id = post.id;
    let post = post.clone();
    let permits = permits.clone();
    pending.insert(
        id,
        tokio::spawn(async move {
            let _permit = permits.acquire_owned().await.ok();
            fetch_preview(&post).await
        }),
    );
}

fn ready_status(post: &Post, path: &Path) -> String {
    let size = std::fs::metadata(path)
        .map(|m| m.len())
        .unwrap_or_else(|_| post.file_size.max(0) as u64);
    format!(
        "Ready · {} · {} × {} px · {}",
        display_ext(post),
        post.width,
        post.height,
        human_bytes(size)
    )
}

fn human_bytes(n: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if n >= GB {
        format!("{:.2} GB", n as f64 / GB as f64)
    } else if n >= MB {
        format!("{:.1} MB", n as f64 / MB as f64)
    } else if n >= KB {
        format!("{:.1} KB", n as f64 / KB as f64)
    } else {
        format!("{n} B")
    }
}

/// Quick downloads live in a dedicated subfolder of the configured download
/// directory (the default is `~/Pictures/Gelbooru`), so they never mix with a
/// user's curated library.
fn quick_download_dir(cfg: &Config) -> PathBuf {
    cfg.download_dir().join("quick-downloads")
}

/// Render a path with the home directory abbreviated to `~` for the footer.
fn friendly_path(path: &Path) -> String {
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        if let Ok(rest) = path.strip_prefix(&home) {
            return format!("~/{}", rest.display());
        }
    }
    path.display().to_string()
}

/// Remove a transient queue message once its download has finished, so the
/// footer does not keep saying "Queued download..." next to the saved result.
fn clear_download_status(status: &mut String, post_id: i64) {
    let queued = format!("Queued download for post #{}...", post_id);
    let active = format!("Post #{} is already downloading", post_id);
    if status == &queued || status == &active {
        status.clear();
    }
}

fn quick_download_status(outcome: &DownloadOutcome) -> String {
    if let Some(err) = &outcome.error {
        if err.to_ascii_lowercase().contains("cancel") {
            return "Download cancelled".to_string();
        }
        return format!("Download failed: {err}");
    }
    if outcome.skipped {
        return format!("Already downloaded: {}", friendly_path(&outcome.path));
    }
    let name = outcome
        .path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file");
    let dir = outcome.path.parent().unwrap_or(&outcome.path);
    format!(
        "Saved {} ({}) to {}",
        name,
        human_bytes(outcome.size),
        friendly_path(dir)
    )
}

/// Place the full-resolution image inside `rect` using kitty's graphics
/// protocol.  `--place` fits (and centers) the image inside the rectangle while
/// keeping its aspect ratio.
///
/// The child process is polled with a timeout so a non-graphics terminal (or a
/// decoder that hangs) can never wedge the whole TUI.
fn place_image(post: &Post, path: &Path, rect: Rect) -> Result<(), String> {
    if rect.width < 2 || rect.height < 2 {
        return Err("Terminal is too small for an image preview".to_string());
    }
    let path = path
        .to_str()
        .ok_or_else(|| "Preview path is not valid UTF-8".to_string())?;
    let place = format!("{}x{}@{}x{}", rect.width, rect.height, rect.x, rect.y);

    let mut cmd = Command::new("kitty");
    cmd.arg("+kitten")
        .arg("icat")
        .arg("--scale-up")
        .arg("--place")
        .arg(&place);

    cmd.arg(path).stdout(Stdio::inherit()).stderr(Stdio::null());

    let _ = crossterm::execute!(io::stdout(), crossterm::cursor::Hide);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Could not start kitty icat: {e}"))?;

    let result = wait_child(&mut child, ICAT_TIMEOUT);
    let _ = crossterm::execute!(io::stdout(), crossterm::cursor::Hide);

    match result {
        Ok(true) => Ok(()),
        Ok(false) => Err(format!(
            "kitty icat could not display {} - press o to open the post",
            display_ext(post)
        )),
        Err(_) => {
            Err("Timed out while displaying the image - press o to open the post".to_string())
        }
    }
}

/// Wait for a child process without ever blocking the TUI forever.
fn wait_child(child: &mut std::process::Child, timeout: Duration) -> Result<bool, String> {
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status.success()),
            Ok(None) => {
                if started.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("timed out".to_string());
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

fn clear_images() {
    let child = Command::new("kitty")
        .args(["+kitten", "icat", "--clear"])
        .stdout(Stdio::inherit())
        .stderr(Stdio::null())
        .spawn();
    if let Ok(mut child) = child {
        let _ = wait_child(&mut child, Duration::from_secs(5));
    }
}

async fn fetch_preview(post: &Post) -> Result<(PathBuf, bool), String> {
    fetch_preview_to(post, &preview_path(post)).await
}

/// Download a preview into `target`.  Split out from `fetch_preview` so tests
/// can use a temporary cache file.
async fn fetch_preview_to(post: &Post, target: &Path) -> Result<(PathBuf, bool), String> {
    // Returns (path, used_sample): used_sample indicates if preview is lower-res

    if post.file_url.trim().is_empty() {
        return Err("This post has no file URL".to_string());
    }

    // Determine which URL to use - prefer sample for large images to load faster
    // Gelbooru provides: file_url (full), sample_url (~1000px), preview_url (thumbnail)
    let used_sample = should_use_sample(post) && !post.sample_url.trim().is_empty();
    let file_url = if used_sample {
        post.sample_url.clone()
    } else {
        post.file_url.clone()
    };

    let client = preview_client()?;

    // Gelbooru hotlink protection redirects to hotlink.php; follow manually so
    // the Referer header is preserved on every hop.
    let mut url = file_url.clone();
    let mut response = None;
    for _ in 0..8 {
        let resp = client
            .get(&url)
            .header(reqwest::header::REFERER, "https://gelbooru.com/")
            .send()
            .await
            .map_err(|e| format!("Download failed: {e}"))?;
        if resp.status().is_redirection() {
            if let Some(loc) = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
            {
                url = resolve_url(&url, loc);
                continue;
            }
        }
        response = Some(resp);
        break;
    }

    let resp = response.ok_or_else(|| "Too many redirects".to_string())?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("HTTP {status}"));
    }

    if let Some(ct) = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
    {
        if !ct.is_empty()
            && !ct.starts_with("image/")
            && !ct.starts_with("video/")
            && ct != "application/octet-stream"
        {
            return Err(format!(
                "Server returned {ct} instead of media - press o to open the post"
            ));
        }
    }

    if let Some(len) = resp.content_length() {
        if len > MAX_PREVIEW_BYTES {
            return Err(format!(
                "File is too large to preview ({}) - press o to open the post",
                human_bytes(len)
            ));
        }
    }

    let dir = target
        .parent()
        .ok_or_else(|| "Invalid preview path".to_string())?;
    std::fs::create_dir_all(dir).map_err(|e| format!("Could not create preview cache: {e}"))?;

    let tmp = preview_tmp_path(target);
    let _ = std::fs::remove_file(&tmp);

    match stream_to_file(resp, &tmp).await {
        Ok(()) => {
            std::fs::rename(&tmp, target).map_err(|e| {
                let _ = std::fs::remove_file(&tmp);
                format!("Could not save preview: {e}")
            })?;
            Ok((target.to_path_buf(), used_sample))
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

async fn stream_to_file(resp: reqwest::Response, tmp: &Path) -> Result<(), String> {
    let mut file = tokio::fs::File::create(tmp)
        .await
        .map_err(|e| format!("Could not create preview cache: {e}"))?;
    let mut stream = resp.bytes_stream();
    let mut written: u64 = 0;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Download failed: {e}"))?;
        written = written.saturating_add(chunk.len() as u64);
        if written > MAX_PREVIEW_BYTES {
            return Err(
                "File is too large to preview (limit 128 MB) - press o to open the post"
                    .to_string(),
            );
        }
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("Could not save preview: {e}"))?;
    }
    file.flush()
        .await
        .map_err(|e| format!("Could not save preview: {e}"))?;

    if written == 0 {
        return Err("Server returned an empty file".to_string());
    }
    Ok(())
}

fn resolve_url(base: &str, location: &str) -> String {
    if location.starts_with("http://") || location.starts_with("https://") {
        return location.to_string();
    }
    url::Url::parse(base)
        .ok()
        .and_then(|b| b.join(location).ok())
        .map(|u| u.to_string())
        .unwrap_or_else(|| location.to_string())
}

/// Fetch one page of search results.  Used by the viewer to grow the loaded
/// window in either direction.
async fn load_page(
    client: &GelbooruClient,
    query: &str,
    per_page: usize,
    page: usize,
) -> Result<Vec<Post>, String> {
    client
        .search_page(query, per_page as u32, page as u32, None, None)
        .await
        .map(|(posts, _)| posts)
        .map_err(|e| e.to_string())
}

/// Human-readable status line after loading an adjacent page: the total
/// result count when the API reports one, otherwise just the window size.
fn loaded_status(total: i64, page: usize, shown: usize) -> String {
    if total > 0 {
        format!("Loaded page {page} - {shown} of {total} shown")
    } else {
        format!("Loaded page {page} - {shown} shown")
    }
}

/// Status line for the "nothing more in this direction" case.
fn end_status(total: i64, shown: usize) -> String {
    if total > 0 {
        format!("End of results - {shown} of {total} shown")
    } else {
        format!("End of results - {shown} shown")
    }
}

fn copy_to_clipboard(text: &str) -> bool {
    use std::io::Write;

    let candidates: &[(&str, &[&str])] = &[
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["--clipboard", "--input"]),
    ];

    for (cmd, args) in candidates {
        let child = Command::new(cmd)
            .args(*args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let mut child = match child {
            Ok(c) => c,
            Err(_) => continue,
        };
        if let Some(mut stdin) = child.stdin.take() {
            if stdin.write_all(text.as_bytes()).is_err() {
                let _ = child.kill();
                let _ = child.wait();
                continue;
            }
            let _ = stdin.flush();
            // Dropping stdin here sends EOF, which clipboard tools need.
        }
        if wait_child(&mut child, Duration::from_secs(3)).unwrap_or(false) {
            return true;
        }
    }
    false
}

fn open_post(post: &Post) -> Result<(), String> {
    let url = format!(
        "https://gelbooru.com/index.php?page=post&s=view&id={}",
        post.id
    );
    Command::new("xdg-open")
        .arg(&url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not open browser: {e}"))
}

/// Entries worth showing in the footer: queued and actively downloading ones.
/// Finished results are shown separately via `combine_download_status`, so a
/// long queue still lists the jobs that are still moving.
fn visible_download_entries(entries: &[DownloadEntry]) -> Vec<&DownloadEntry> {
    entries
        .iter()
        .filter(|e| {
            matches!(
                e.status,
                DownloadStatus::Queued | DownloadStatus::Downloading
            )
        })
        .collect()
}

/// Reap finished download tasks without blocking on the ones still running.
async fn reap_downloads(
    handles: &mut Vec<tokio::task::JoinHandle<DownloadOutcome>>,
) -> Vec<DownloadOutcome> {
    let mut finished = Vec::new();
    let mut pending = Vec::new();
    for handle in handles.drain(..) {
        if handle.is_finished() {
            match handle.await {
                Ok(outcome) => finished.push(outcome),
                Err(e) => tracing::warn!("download task failed: {e}"),
            }
        } else {
            pending.push(handle);
        }
    }
    *handles = pending;
    finished
}

fn combine_download_status(notice: &str, entries: &[&DownloadEntry]) -> String {
    let progress = format_download_progress(entries);
    match (notice.is_empty(), progress.is_empty()) {
        (true, _) => progress,
        (false, true) => notice.to_string(),
        (false, false) => format!("{notice}  ·  {progress}"),
    }
}

/// Format download progress for display in the status bar.
/// Shows progress for up to 3 downloads, then truncates with count.
fn format_download_progress(entries: &[&DownloadEntry]) -> String {
    let mut parts = Vec::new();
    for e in entries.iter().take(3) {
        let progress_str = match e.status {
            DownloadStatus::Queued => "queued".to_string(),
            DownloadStatus::Downloading => {
                if e.total > 0 {
                    format!(
                        "{}% ({}/{})",
                        e.progress,
                        human_bytes(e.bytes),
                        human_bytes(e.total)
                    )
                } else {
                    human_bytes(e.bytes)
                }
            }
            DownloadStatus::Completed => "done".to_string(),
            DownloadStatus::Failed => "failed".to_string(),
            DownloadStatus::Cancelled => "cancelled".to_string(),
        };
        parts.push(format!("#{}: {}", e.post_id, progress_str));
    }

    if entries.len() > 3 {
        parts.push(format!("(+{} more)", entries.len() - 3));
    }

    parts.join(", ")
}

fn rating_color(rating: &str) -> Color {
    match rating.to_lowercase().as_str() {
        "general" | "safe" | "s" => Color::Green,
        "sensitive" => Color::Yellow,
        "questionable" | "q" => Color::LightYellow,
        "explicit" | "e" | "nsfw" => Color::Red,
        _ => Color::Gray,
    }
}

fn rating_border(rating: &str) -> Color {
    match rating.to_lowercase().as_str() {
        "general" | "safe" | "s" => Color::Rgb(72, 116, 92),
        "sensitive" => Color::Rgb(132, 122, 64),
        "questionable" | "q" => Color::Rgb(140, 112, 58),
        "explicit" | "e" | "nsfw" => Color::Rgb(142, 68, 80),
        _ => BORDER,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn post(url: &str) -> Post {
        Post {
            file_url: url.to_string(),
            ..Post::default()
        }
    }

    #[test]
    fn classifies_video_files_as_video() {
        for url in [
            "https://img4.gelbooru.com/images/ab/cd/123.mp4",
            "https://img4.gelbooru.com/images/ab/cd/123.webm",
            "https://img4.gelbooru.com/images/ab/cd/123.mkv?x=1",
        ] {
            assert!(matches!(media_kind(&post(url)), MediaKind::Video), "{url}");
        }
    }

    #[test]
    fn gifs_are_previewable_in_kitty() {
        let kind = media_kind(&post("https://img4.gelbooru.com/images/ab/cd/123.gif"));
        assert!(matches!(kind, MediaKind::Gif));
        assert!(is_previewable(kind));
    }

    #[test]
    fn static_images_are_previewable() {
        for url in [
            "https://img4.gelbooru.com/images/ab/cd/123.jpg",
            "https://img4.gelbooru.com/images/ab/cd/123.png",
            "https://img4.gelbooru.com/images/ab/cd/123.webp",
        ] {
            assert!(is_previewable(media_kind(&post(url))), "{url}");
        }
    }

    #[test]
    fn unknown_extensions_are_not_previewed() {
        let kind = media_kind(&post("https://img4.gelbooru.com/images/ab/cd/123.exe"));
        assert!(matches!(kind, MediaKind::Other));
    }

    #[test]
    fn wrapped_lines_never_exceed_width() {
        let text = "one two three four five six seven eight nine ten artist:some_long_name";
        for width in [4usize, 8, 12, 20] {
            for line in wrap_text(text, width) {
                assert!(
                    line.chars().count() <= width,
                    "width {width}: {line:?} is too long"
                );
            }
        }
    }

    #[test]
    fn viewer_layout_renders_at_many_sizes() {
        use ratatui::backend::TestBackend;

        let post = Post {
            id: 123456,
            rating: "explicit".into(),
            width: 1920,
            height: 1080,
            score: 42,
            source: "https://example.com/a/very/long/source/url/that/needs/wrapping".into(),
            tags: (0..80)
                .map(|i| format!("artist:artist_{i}_with_a_long_name"))
                .collect::<Vec<_>>()
                .join(" "),
            file_url: "https://img4.gelbooru.com/images/ab/cd/123.jpg".into(),
            file_size: 2_500_000,
            ..Post::default()
        };
        let posts = [post];

        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        for (w, h) in [(80u16, 24u16), (120, 40), (50, 14), (200, 60), (20, 8)] {
            terminal.resize(Rect::new(0, 0, w, h)).unwrap();
            for phase in [
                Phase::Loading,
                Phase::Ready,
                Phase::Error(
                    "a very long error message that should wrap nicely in the panel".into(),
                ),
                Phase::Unsupported("Video files cannot be previewed in the terminal.".into()),
            ] {
                terminal
                    .draw(|f| {
                        let reg = regions(f.area());
                        draw_header(f, &posts[0], 0, 1, reg.header);
                        let _ = draw_image_block(f, &posts[0], &phase, reg.image);
                        let _ = draw_details_and_tags(f, &posts[0], reg.details, reg.tags, 3);
                        draw_footer(f, &phase, "Ready", "", reg.status, reg.hints);
                    })
                    .unwrap();
            }
        }
    }

    #[test]
    fn long_tags_are_wrapped_not_lost() {
        let long = "x".repeat(87);
        let text = format!("prefix {long} suffix");
        let lines = tag_lines(&text, 12);
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert!(joined.contains(&long), "long tag was truncated: {joined}");
        assert!(joined.contains("suffix"));
    }

    #[test]
    fn quick_download_dir_is_inside_configured_directory() {
        let mut cfg = Config::default();
        cfg.downloads.directory = "/tmp/gelbooru-test".to_string();
        assert_eq!(
            quick_download_dir(&cfg),
            PathBuf::from("/tmp/gelbooru-test/quick-downloads")
        );
    }

    #[test]
    fn quick_download_status_is_human_readable() {
        let outcome = DownloadOutcome {
            post_id: 1,
            path: PathBuf::from("/tmp/quick-downloads/1_abc.png"),
            size: 2048,
            hash: String::new(),
            skipped: false,
            error: None,
        };
        let saved = quick_download_status(&outcome);
        assert!(saved.contains("Saved"), "{saved}");
        assert!(saved.contains("2.0 KB"), "{saved}");

        let skipped = DownloadOutcome {
            skipped: true,
            ..outcome.clone()
        };
        assert!(quick_download_status(&skipped).starts_with("Already downloaded"));

        let failed = DownloadOutcome {
            error: Some("cancelled".to_string()),
            ..outcome
        };
        assert_eq!(quick_download_status(&failed), "Download cancelled");
    }

    #[test]
    fn truncation_is_unicode_safe() {
        let s = "タグとタグとタグ";
        let out = truncate_chars(s, 5);
        assert_eq!(out.chars().count(), 5);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn preview_temp_paths_are_unique_and_beside_the_target() {
        let target = Path::new("/tmp/previews/42_preview.jpg");
        let a = preview_tmp_path(target);
        let b = preview_tmp_path(target);
        assert_ne!(a, b);
        assert_ne!(a, target);
        assert_eq!(a.parent(), target.parent());
        assert!(a.to_string_lossy().ends_with(".part"));
    }

    #[test]
    fn preview_cache_requires_a_non_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("42_preview.jpg");
        assert!(!preview_is_cached(&path));
        std::fs::write(&path, b"x").unwrap();
        assert!(preview_is_cached(&path));
    }

    #[test]
    fn preview_http_client_is_reused() {
        let first = preview_client().unwrap();
        let second = preview_client().unwrap();
        assert!(std::ptr::eq(first, second));
    }

    #[tokio::test]
    async fn load_preview_reuses_a_finished_prefetch() {
        let post = Post {
            id: 7,
            file_url: "https://example.invalid/image.jpg".into(),
            ..Post::default()
        };
        let expected = PathBuf::from("/tmp/prefetched.jpg");
        let task = tokio::spawn({
            let path = expected.clone();
            async move { Ok((path, true)) }
        });

        let (path, _) = load_preview(&post, Some(task)).await.unwrap();
        assert_eq!(path, expected);
    }

    #[test]
    fn download_progress_lists_multiple_active_entries() {
        let entries = vec![
            DownloadEntry {
                post_id: 1,
                status: DownloadStatus::Downloading,
                progress: 50,
                total: 100,
                bytes: 50,
                error: None,
            },
            DownloadEntry {
                post_id: 2,
                status: DownloadStatus::Queued,
                progress: 0,
                total: 0,
                bytes: 0,
                error: None,
            },
            DownloadEntry {
                post_id: 3,
                status: DownloadStatus::Downloading,
                progress: 25,
                total: 100,
                bytes: 25,
                error: None,
            },
            DownloadEntry {
                post_id: 4,
                status: DownloadStatus::Completed,
                progress: 100,
                total: 100,
                bytes: 100,
                error: None,
            },
        ];

        let visible = visible_download_entries(&entries);
        assert_eq!(visible.len(), 3);
        let summary = format_download_progress(&visible);
        assert!(summary.contains("#1: 50%"));
        assert!(summary.contains("#2: queued"));
        assert!(summary.contains("#3: 25%"));
        assert!(!summary.contains("#4:"));

        let unknown_total = DownloadEntry {
            post_id: 9,
            status: DownloadStatus::Downloading,
            progress: 0,
            total: 0,
            bytes: 2048,
            error: None,
        };
        let text = format_download_progress(&[&unknown_total]);
        assert_eq!(text, "#9: 2.0 KB");
        assert!(!text.contains("KBB"), "{text}");
    }

    #[test]
    fn finishing_a_download_clears_its_stale_queue_message() {
        let mut queued = "Queued download for post #7...".to_string();
        clear_download_status(&mut queued, 7);
        assert!(queued.is_empty(), "{queued:?}");

        let mut active = "Post #7 is already downloading".to_string();
        clear_download_status(&mut active, 7);
        assert!(active.is_empty(), "{active:?}");

        // A message for a different post (or an unrelated status) is kept.
        let mut other = "Queued download for post #8...".to_string();
        clear_download_status(&mut other, 7);
        assert_eq!(other, "Queued download for post #8...");

        let mut copied = "Copied 5 tags".to_string();
        clear_download_status(&mut copied, 7);
        assert_eq!(copied, "Copied 5 tags");
    }

    #[test]
    fn finished_download_notice_is_combined_with_live_progress() {
        assert_eq!(combine_download_status("Saved file", &[]), "Saved file");

        let entry = DownloadEntry {
            post_id: 4,
            status: DownloadStatus::Downloading,
            progress: 10,
            total: 100,
            bytes: 10,
            error: None,
        };
        let summary = combine_download_status("Saved file", &[&entry]);
        assert!(summary.starts_with("Saved file"));
        assert!(summary.contains("#4:"));
    }

    #[tokio::test]
    async fn fetch_preview_writes_the_cache_file_atomically() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let body = b"\x89PNG\r\n\x1a\npreview-bytes".to_vec();
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "image/png")
                    .set_body_bytes(body.clone()),
            )
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("42_preview.png");
        let post = Post {
            id: 42,
            file_url: format!("{}/image.png", server.uri()),
            width: 800,
            height: 600,
            ..Post::default()
        };

        let (path, _) = fetch_preview_to(&post, &target).await.unwrap();
        assert_eq!(path, target);
        assert_eq!(std::fs::read(&target).unwrap(), body);
        // Only the final file is left; the temporary file was renamed away.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
