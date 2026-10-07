use std::time::Duration;

use eframe::egui;

use crate::fs::transfer::{Mode, Progress};

pub struct Status<'a> {
    pub item_count: usize,
    pub hidden_count: usize,
    pub show_hidden: bool,
    pub loading: bool,
    pub last_elapsed: Option<Duration>,
    pub error: Option<&'a str>,
    /// Progress of the running copy or move, already formatted.
    pub job: Option<String>,
    pub notice: Option<&'a str>,
}

pub fn show(ui: &mut egui::Ui, status: &Status<'_>) {
    ui.horizontal(|ui| {
        ui.label(format!("{} items", status.item_count));
        if status.hidden_count > 0 && !status.show_hidden {
            ui.weak(format!("({} hidden, Ctrl+H to show)", status.hidden_count));
        }
        if let Some(elapsed) = status.last_elapsed {
            ui.separator();
            ui.label(format!("listed in {:.2} ms", elapsed.as_secs_f64() * 1000.0));
        }
        if status.loading {
            ui.separator();
            ui.spinner();
            ui.label("Loading…");
        }
        if let Some(job) = &status.job {
            ui.separator();
            ui.spinner();
            ui.label(job);
            ui.weak("Esc to cancel");
        }
        if let Some(notice) = status.notice {
            ui.separator();
            ui.label(notice);
        }
        if let Some(error) = status.error {
            ui.separator();
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
    });
}

/// "Copying 3/10 — 45%": files done of total, then share of bytes.
pub fn progress_text(mode: Mode, progress: &Progress) -> String {
    let verb = match mode {
        Mode::Copy => "Copying",
        Mode::Move => "Moving",
    };
    format!("{verb} {}/{} — {}%", progress.files_done, progress.files_total, progress.percent())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_reads_files_then_percent() {
        let p = Progress { files_done: 3, files_total: 10, bytes_done: 45, bytes_total: 100 };
        assert_eq!(progress_text(Mode::Copy, &p), "Copying 3/10 — 45%");
        assert_eq!(progress_text(Mode::Move, &Progress::default()), "Moving 0/0 — 100%");
    }
}
