#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod dsp;
mod prefs;
mod probe;
mod track;
mod visual;
mod worker;
use ferrite_design::prelude::*;
use gpui::{
    App, AppContext as _, Context, Entity, ExternalPaths, IntoElement, KeyBinding,
    PathPromptOptions, Render, Subscription, Task, Window, div, px, size,
};
use std::{path::PathBuf, sync::Arc};
gpui::actions!(prism, [OpenSong]);
struct Prism {
    palette: Entity<CommandPalette>,
    _appearance: Subscription,
    track: Option<Arc<track::Track>>,
    report: Option<dsp::Report>,
    status: String,
    busy: bool,
    generation: u64,
    job: Option<Task<()>>,
    mode: usize,
}
impl Prism {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut app = Self {
            palette: cx.new(|cx| CommandPalette::new(window, cx)),
            _appearance: theme::follow_system(window),
            track: None,
            report: None,
            status: "Drop a bounce or open one with Ctrl+O".into(),
            busy: false,
            generation: 0,
            job: None,
            mode: 0,
        };
        let weak = cx.weak_entity();
        app.palette.update(cx, |p, cx| {
            let mut commands = vec![
                command("Analyze song")
                    .group("Prism")
                    .shortcut("Ctrl+O")
                    .on_run(move |_, cx| {
                        let _ = weak.update(cx, |this, cx| this.open(cx));
                    }),
            ];
            for scheme in SCHEMES {
                commands.push(
                    command(format!("Scheme: {}", scheme.name))
                        .group("Theme")
                        .on_run(move |_, cx| prefs::scheme(scheme, cx)),
                );
            }
            commands.push(
                command("Toggle light / dark")
                    .group("Theme")
                    .on_run(|w, cx| {
                        prefs::appearance(
                            if theme::appearance(cx) == Appearance::Light {
                                Appearance::Dark
                            } else {
                                Appearance::Light
                            },
                            w,
                            cx,
                        )
                    }),
            );
            p.set_commands(commands, cx);
        });
        if let Some(path) = cli_path() {
            app.load(path, cx);
        }
        app
    }
    fn open(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Analyze a mono or stereo song".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ = this.update(cx, |this, cx| this.load(path, cx));
            }
        })
        .detach();
    }
    fn load(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.generation += 1;
        let generation = self.generation;
        self.busy = true;
        self.status = "Decoding and analyzing audio...".into();
        self.job = Some(cx.spawn(async move |this, cx| {
            let result = worker::run(move || {
                let track = track::load(&path)?;
                let report = dsp::analyze(&track);
                Ok((track, report))
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.busy = false;
                match result {
                    Ok((track, report)) => {
                        this.status = format!(
                            "{} Hz / {} stereo frames / {} spectrum windows",
                            track.rate,
                            track.frames.len(),
                            report.windows
                        );
                        this.track = Some(track);
                        this.report = Some(report);
                    }
                    Err(e) => this.status = e,
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
}
impl Render for Prism {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let content = if let (Some(track), Some(report)) = (&self.track, &self.report) {
            probe::record(format!(
                "Analyzed {} frames; {} windows; RMS {:?}; correlation {:?}",
                track.frames.len(),
                report.windows,
                report.levels.rms,
                report.levels.correlation
            ));
            let correlation = report
                .levels
                .correlation
                .map_or("UNDEFINED (SILENT CHANNEL)".into(), |v| format!("{v:+.3}"));
            let crest = |ch| {
                if report.levels.rms[ch] > 0. {
                    dsp::db(report.levels.peak[ch] / report.levels.rms[ch])
                } else {
                    "N/A".into()
                }
            };
            let graph = if self.mode == 0 {
                visual::spectrum(dsp::bars(&report.spectrum), cx).into_any_element()
            } else {
                let points = track
                    .frames
                    .iter()
                    .step_by((track.frames.len() / 2048).max(1))
                    .take(2048)
                    .copied()
                    .collect();
                visual::trace(points, true, cx).into_any_element()
            };
            div().flex().flex_col().size_full().min_h(px(640.)).gap_3()
                .child(div().flex().items_center().gap_3().child(div().flex_1().min_w_0().overflow_hidden().child(track.name.clone()))
                    .child(segmented("graph-mode").option("Spectrum").option("Stereo").selected(self.mode).on_select(cx.listener(|this,index,_,cx|{this.mode = *index;cx.notify();}))))
                .child(div().flex().flex_row().flex_wrap().gap_3()
                    .child(panel("Levels").flex_1().min_w(px(280.)).child(property_list()
                        .row("Sample peak L / R",format!("{} / {} dBFS",dsp::db(report.levels.peak[0]),dsp::db(report.levels.peak[1])))
                        .row("RMS L / R",format!("{} / {} dBFS",dsp::db(report.levels.rms[0]),dsp::db(report.levels.rms[1])))
                        .row("Crest factor L / R",format!("{} / {} dB",crest(0),crest(1)))))
                    .child(panel("Stereo / headroom").flex_1().min_w(px(280.)).child(property_list()
                        .row("Channel correlation",correlation).row("Samples at full scale",report.levels.clipped.to_string())
                        .row("Duration",track::time(track.seconds())))))
                .child(panel(if self.mode==0{"Frequency balance"}else{"Stereo vectorscope"}).meta(if self.mode==0{"30 Hz - 20 kHz / -80 to 0 dBFS"}else{"L-R / L+R / UP TO 2048 SAMPLED POINTS"}).flex_1().min_h(px(160.)).child(div().flex_1().min_h_0().child(graph)))
                .child(panel("Whole-song waveform").meta("MIN / MAX ENVELOPE").child(div().w_full().h(px(100.)).child(visual::overview(report.waveform.clone(),cx))))
                .child(div().body(text::SM).text_color(hsla(p.fg_dim)).child(format!("Levels use every decoded sample. Spectrum averages {} evenly spaced FFT windows. Sample peak and RMS are not true peak or LUFS.",report.windows)))
                .into_any_element()
        } else {
            empty_state("Open a song","Drop a mono or stereo WAV, MP3, FLAC, OGG, AAC or M4A bounce here to analyze it. No audio is played.",window,cx).into_any_element()
        };
        window_frame().child(power_on_in(
            "power",
            div()
                .size_full()
                .flex()
                .flex_col()
                .bg(hsla(p.bg))
                .text_color(hsla(p.fg))
                .body(text::BASE)
                .on_action(cx.listener(|this, _: &TogglePalette, w, cx| {
                    this.palette.update(cx, |p, cx| p.toggle(w, cx))
                }))
                .on_action(cx.listener(|this, _: &OpenSong, _, cx| this.open(cx)))
                .child(self.palette.clone())
                .child(
                    title_bar("Prism")
                        .child(
                            Button::new("menu")
                                .icon(Icon::Menu)
                                .ghost()
                                .small()
                                .on_click(cx.listener(|this, _, w, cx| {
                                    this.palette.update(cx, |p, cx| p.toggle(w, cx))
                                })),
                        )
                        .child(
                            Button::new("open")
                                .label("Analyze song")
                                .icon(Icon::Folder)
                                .ghost()
                                .small()
                                .disabled(self.busy)
                                .on_click(cx.listener(|this, _, _, cx| this.open(cx))),
                        ),
                )
                .child(
                    scroll_area("analysis-scroll").flex_1().min_h_0().child(
                        div()
                            .id("analysis-content")
                            .size_full()
                            .min_h(px(if self.report.is_some() { 680. } else { 0. }))
                            .flex()
                            .flex_col()
                            .p_4()
                            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                                if let Some(path) = paths.paths().first() {
                                    this.load(path.clone(), cx);
                                }
                            }))
                            .child(content),
                    ),
                )
                .child(
                    status_bar()
                        .left_live(self.status.clone())
                        .right(theme::scheme(cx).name.to_uppercase()),
                ),
        ))
    }
}
fn cli_path() -> Option<PathBuf> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    args.iter()
        .position(|a| a == "--file")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
}
fn main() {
    if std::env::args().any(|a| a == "--version") {
        println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
        return;
    }
    if std::env::args().any(|a| a == "--smoke") {
        let result = cli_path()
            .ok_or("Use --smoke --file PATH".into())
            .and_then(|p| track::load(&p));
        match result {
            Ok(t) => {
                let r = dsp::analyze(&t);
                println!(
                    "{}",
                    serde_json::json!({"file":t.name,"rate":t.rate,"frames":t.frames.len(),"seconds":t.seconds(),"peak":r.levels.peak,"rms":r.levels.rms,"correlation":r.levels.correlation,"clipped_samples":r.levels.clipped,"spectrum_windows":r.windows})
                );
            }
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        return;
    }
    gpui_platform::application().run(|cx: &mut App| {
        prefs::init(cx);
        cx.bind_keys([
            KeyBinding::new("ctrl-shift-p", TogglePalette, None),
            KeyBinding::new("ctrl-o", OpenSong, None),
        ]);
        let options =
            chrome::remembered_window_options("prism", "Prism", size(px(1180.), px(900.)), cx);
        cx.open_window(options, |w, cx| {
            chrome::square_corners(w);
            chrome::remember_window("prism", w, cx);
            chrome::power_off_on_close(w, cx);
            cx.new(|cx| Prism::new(w, cx))
        })
        .expect("Couldn't open Prism");
        cx.activate(true);
    });
}
