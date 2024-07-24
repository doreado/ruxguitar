use iced::widget::{column, horizontal_space, pick_list, row, text};
use iced::{keyboard, stream, Alignment, Element, Subscription, Task, Theme};
use std::borrow::Cow;
use std::fmt::Display;

use crate::audio::midi_player::AudioPlayer;
use crate::parser::song_parser::{parse_gp_data, GpVersion, Song};
use crate::ui::icons::{open_icon, pause_icon, play_icon, solo_icon, stop_icon};
use crate::ui::picker::{open_file, PickerError};
use crate::ui::tablature::Tablature;
use crate::ui::utils::{action_gated, action_toggle, untitled_text_table_box};
use crate::ApplicationArgs;
use iced::futures::{SinkExt, Stream};
use iced::keyboard::key::Named::Space;
use iced::widget::scrollable::{scroll_to, AbsoluteOffset, Id};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use tokio::sync::watch::{Receiver, Sender};
use tokio::sync::Mutex;

const ICONS_FONT: &[u8] = include_bytes!("../../resources/icons.ttf");

pub struct RuxApplication {
    song_info: Option<SongDisplayInfo>,         // parsed song
    track_selection: TrackSelection,            // selected track
    all_tracks: Vec<TrackSelection>,            // all possible tracks
    tablature: Option<Tablature>,               // loaded tablature
    audio_player: Option<AudioPlayer>,          // audio player
    tab_file_is_loading: bool,                  // file loading flag in progress
    sound_font_file: Option<PathBuf>,           // sound font file
    beat_sender: Arc<Sender<usize>>,            // beat notifier
    beat_receiver: Arc<Mutex<Receiver<usize>>>, // beat receiver
}

#[derive(Debug)]
struct SongDisplayInfo {
    name: String,
    artist: String,
    gp_version: GpVersion,
    file_name: String,
}

impl SongDisplayInfo {
    fn new(song: &Song, file_name: String) -> Self {
        Self {
            name: song.song_info.name.clone(),
            artist: song.song_info.artist.clone(),
            gp_version: song.version,
            file_name,
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct TrackSelection {
    index: usize,
    name: String,
}

impl TrackSelection {
    fn new(index: usize, name: String) -> Self {
        Self { index, name }
    }
}

impl Display for TrackSelection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} - {}", self.index + 1, self.name)
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    OpenFile,                                           // open file dialog
    FileOpened(Result<(Vec<u8>, String), PickerError>), // file content & file name
    TrackSelected(TrackSelection),                      // track selection
    FocusMeasure(usize), // used when clicking on measure in tablature
    FocusTick(usize),    // focus on a specific tick in the tablature
    PlayPause,           // toggle play/pause
    StopPlayer,          // stop playback
    ToggleSolo,          // toggle solo mode
}

impl RuxApplication {
    fn new(sound_font_file: Option<PathBuf>) -> Self {
        let (beat_sender, beat_receiver) = tokio::sync::watch::channel(0);
        Self {
            song_info: None,
            track_selection: TrackSelection::default(),
            all_tracks: vec![],
            tablature: None,
            audio_player: None,
            tab_file_is_loading: false,
            sound_font_file,
            beat_receiver: Arc::new(Mutex::new(beat_receiver)),
            beat_sender: Arc::new(beat_sender),
        }
    }

    pub fn start(args: ApplicationArgs) -> iced::Result {
        iced::application(
            RuxApplication::title,
            RuxApplication::update,
            RuxApplication::view,
        )
        .subscription(RuxApplication::subscription)
        .default_font(iced::Font::MONOSPACE)
        .theme(RuxApplication::theme)
        .font(ICONS_FONT)
        .window_size((1150.0, 768.0))
        .centered()
        .antialiasing(!args.no_antialiasing)
        .run_with(move || {
            (
                RuxApplication::new(args.sound_font_bank.clone()),
                Task::none(),
            )
        })
    }

    fn title(&self) -> String {
        match &self.song_info {
            Some(song_info) => format!("Ruxguitar - {}", song_info.file_name),
            None => String::from("Ruxguitar - untitled"),
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::TrackSelected(selection) => {
                if let Some(tablature) = self.tablature.as_mut() {
                    tablature.update_track(selection.index);
                }
                self.track_selection = selection;
                Task::none()
            }
            Message::OpenFile => {
                if self.tab_file_is_loading {
                    Task::none()
                } else {
                    self.tab_file_is_loading = true;
                    Task::perform(open_file(), Message::FileOpened)
                }
            }
            Message::FileOpened(result) => {
                self.tab_file_is_loading = false;
                match result {
                    Ok((contents, file_name)) => {
                        if let Ok(song) = parse_gp_data(&contents) {
                            // build all tracks selection
                            let track_selections: Vec<_> = song
                                .tracks
                                .iter()
                                .enumerate()
                                .map(|(index, track)| {
                                    TrackSelection::new(index, track.name.clone())
                                })
                                .collect();
                            self.all_tracks.clone_from(&track_selections);
                            self.song_info = Some(SongDisplayInfo::new(&song, file_name));
                            // select first track by default
                            let default_track = 0;
                            let default_track_selection = track_selections[default_track].clone();
                            self.track_selection = default_track_selection;
                            // share song ownership with tablature and player
                            let song_rc = Rc::new(song);
                            let tablature_scroll_id =
                                Id::new(Cow::Borrowed("tablature-scroll-elements"));
                            let tablature = Tablature::new(
                                song_rc.clone(),
                                default_track,
                                tablature_scroll_id.clone(),
                            );
                            self.tablature = Some(tablature);
                            // stop previous audio player if any
                            if let Some(audio_player) = &mut self.audio_player {
                                audio_player.stop();
                            }
                            // audio player initialization
                            let audio_player = AudioPlayer::new(
                                song_rc.clone(),
                                song_rc.tempo.value,
                                self.sound_font_file.clone(),
                                self.beat_sender.clone(),
                            );
                            self.audio_player = Some(audio_player);
                            // reset tablature scroll
                            scroll_to(tablature_scroll_id, AbsoluteOffset::default())
                        } else {
                            log::warn!("Failed to parse GP file");
                            // TODO show alert popup
                            Task::none()
                        }
                    }
                    Err(err) => {
                        log::warn!("Failed to read GP file: {}", err);
                        // TODO show alert popup
                        Task::none()
                    }
                }
            }
            Message::FocusMeasure(measure_id) => {
                // focus measure in tablature
                if let Some(tablature) = &mut self.tablature {
                    tablature.focus_on_measure(measure_id);
                }
                // focus measure in player
                if let Some(audio_player) = &mut self.audio_player {
                    audio_player.focus_measure(measure_id);
                }
                Task::none()
            }
            Message::FocusTick(tick) => {
                if let Some(tablature) = &mut self.tablature {
                    tablature.focus_on_tick(tick);
                }
                Task::none()
            }
            Message::PlayPause => {
                if let Some(audio_player) = &mut self.audio_player {
                    audio_player.toggle_play();
                }
                Task::none()
            }
            Message::StopPlayer => {
                if let (Some(audio_player), Some(tablature)) =
                    (&mut self.audio_player, &mut self.tablature)
                {
                    // stop audio player
                    audio_player.stop();
                    // reset tablature focus
                    tablature.focus_on_measure(0);
                    // reset tablature scroll
                    scroll_to(tablature.scroll_id.clone(), AbsoluteOffset::default())
                } else {
                    Task::none()
                }
            }
            Message::ToggleSolo => {
                if let Some(audio_player) = &mut self.audio_player {
                    let track = self.track_selection.index;
                    audio_player.toggle_solo_mode(track);
                }
                Task::none()
            }
        }
    }

    fn view(&self) -> Element<Message> {
        let open_file = action_gated(
            open_icon(),
            "Open file",
            (!self.tab_file_is_loading).then_some(Message::OpenFile),
        );

        let player_control = if let Some(audio_player) = &self.audio_player {
            let (icon, message) = if audio_player.is_playing() {
                (pause_icon(), "Pause")
            } else {
                (play_icon(), "Play")
            };
            let play_button = action_gated(icon, message, Some(Message::PlayPause));
            let stop_button = action_gated(stop_icon(), "Stop", Some(Message::StopPlayer));
            row![play_button, stop_button,]
                .spacing(10)
                .align_y(Alignment::Center)
        } else {
            row![horizontal_space()]
        };

        let track_control = if self.all_tracks.is_empty() {
            row![horizontal_space()]
        } else {
            let solo_mode = action_toggle(
                solo_icon(),
                "Solo",
                Message::ToggleSolo,
                self.audio_player
                    .as_ref()
                    .is_some_and(|p| p.solo_track_id().is_some()),
            );

            let track_pick_list = pick_list(
                self.all_tracks.as_slice(),
                Some(&self.track_selection),
                Message::TrackSelected,
            )
            .text_size(14)
            .padding([5, 10]);

            row![solo_mode, track_pick_list,]
                .spacing(10)
                .align_y(Alignment::Center)
        };

        let controls = row![
            open_file,
            horizontal_space(),
            player_control,
            horizontal_space(),
            track_control,
        ]
        .spacing(10)
        .align_y(Alignment::Center);

        let status = row![
            text(if let Some(song) = &self.song_info {
                format!("{} by {}", song.name, song.artist)
            } else {
                String::new()
            }),
            horizontal_space(),
            text(if let Some(song) = &self.song_info {
                format!("{:?}", song.gp_version)
            } else {
                String::new()
            }),
        ]
        .spacing(10);

        let tablature_view = self
            .tablature
            .as_ref()
            .map_or(untitled_text_table_box().into(), |t| t.view());

        column![controls, tablature_view, status,]
            .spacing(20)
            .padding(10)
            .into()
    }

    fn theme(&self) -> Theme {
        Theme::Dark
    }

    fn audio_player_beat_subscription(&self) -> impl Stream<Item = Message> {
        let beat_receiver = self.beat_receiver.clone();
        stream::channel(1, move |mut output| async move {
            let mut receiver = beat_receiver.lock().await;
            loop {
                // get tick from audio player
                let tick = *receiver.borrow_and_update();
                // publish to UI
                output
                    .send(Message::FocusTick(tick))
                    .await
                    .expect("send failed");
                // wait for next beat
                receiver.changed().await.expect("receiver failed");
            }
        })
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut subscriptions = Vec::with_capacity(2);

        // keyboard event subscription
        let keyboard_subscription = keyboard::on_key_press(|key, _modifiers| match key.as_ref() {
            keyboard::Key::Named(Space) => Some(Message::PlayPause),
            _ => None,
        });
        subscriptions.push(keyboard_subscription);

        // next beat notifier subscription
        let audio_player_beat_subscription = self.audio_player_beat_subscription();
        subscriptions.push(Subscription::run_with_id(
            "audio-player-beat",
            audio_player_beat_subscription,
        ));

        Subscription::batch(subscriptions)
    }
}
