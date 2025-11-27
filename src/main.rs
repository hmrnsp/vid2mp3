#![windows_subsystem = "windows"]

use eframe::egui::{self, Color32, ColorImage, CornerRadius, IconData, Stroke, TextureHandle, Vec2};
use rfd::FileDialog;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::process::Command;
use tokio::runtime::Runtime;

fn load_icon() -> Option<Arc<IconData>> {
    let icon_path = "assets/icon.ico";
    match image::open(icon_path) {
        Ok(img) => {
            let img = img.to_rgba8();
            let (width, height) = img.dimensions();
            let pixels = img.into_raw();
            Some(Arc::new(IconData {
                rgba: pixels,
                width: width,
                height: height,
            }))
        }
        Err(_) => None,
    }
}

fn main() -> eframe::Result<()> {
    let rt = Runtime::new().unwrap();

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([300.0, 320.0])
        .with_resizable(false);

    if let Some(icon) = load_icon() {
        viewport = viewport.with_icon(icon);
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "Video to MP3",
        options,
        Box::new(|_cc| Ok(Box::new(App::new(rt)))),
    )
}

struct App {
    runtime: Runtime,
    input_path: Option<PathBuf>,
    output_path: Option<PathBuf>,
    status: Arc<Mutex<Status>>,
    dropped_file: bool,
    info_icon: Option<TextureHandle>,
    show_info_popup: bool,
    video_thumbnail: Option<TextureHandle>,
    thumbnail_path: Arc<Mutex<Option<PathBuf>>>,
    thumbnail_loading: bool,
}

#[derive(Clone)]
enum Status {
    Idle,
    Converting,
    Done,
    Error(String),
}

impl App {
    fn new(runtime: Runtime) -> Self {
        Self {
            runtime,
            input_path: None,
            output_path: None,
            status: Arc::new(Mutex::new(Status::Idle)),
            dropped_file: false,
            info_icon: None,
            show_info_popup: false,
            video_thumbnail: None,
            thumbnail_path: Arc::new(Mutex::new(None)),
            thumbnail_loading: false,
        }
    }

    fn load_icon_from_file(&mut self, ctx: &egui::Context, path: &str) -> Option<TextureHandle> {
        match image::open(path) {
            Ok(img) => {
                println!("Image opened successfully: {}x{}", img.width(), img.height());
                let size = [img.width() as usize, img.height() as usize];
                let img_buffer = img.to_rgba8();
                let pixels = img_buffer.as_flat_samples();
                let color_image = ColorImage::from_rgba_unmultiplied(size, pixels.as_slice());

                Some(ctx.load_texture(
                    "thumbnail",
                    color_image,
                    Default::default()
                ))
            }
            Err(e) => {
                println!("Failed to open image '{}': {}", path, e);
                None
            }
        }
    }

    fn set_input(&mut self, path: PathBuf) {
        let mut output = path.clone();
        output.set_extension("mp3");
        self.output_path = Some(output);
        self.input_path = Some(path.clone());
        self.video_thumbnail = None; // Reset thumbnail when new video is selected
        self.thumbnail_loading = false;
        *self.thumbnail_path.lock().unwrap() = None;
        *self.status.lock().unwrap() = Status::Idle;

        // Start async thumbnail extraction
        self.extract_thumbnail_async(path);
    }

    fn extract_thumbnail_async(&mut self, video_path: PathBuf) {
        use std::fs;

        let thumbnail_path_arc = Arc::clone(&self.thumbnail_path);
        self.thumbnail_loading = true;

        self.runtime.spawn(async move {
            println!("Starting thumbnail extraction for: {:?}", video_path);

            // Create temp directory if it doesn't exist
            let temp_dir = std::env::temp_dir().join("vid2mp3");
            if let Err(e) = fs::create_dir_all(&temp_dir) {
                println!("Failed to create temp dir: {}", e);
                return;
            }
            println!("Temp directory: {:?}", temp_dir);

            // Generate thumbnail path with timestamp to avoid conflicts
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            let thumbnail_file = temp_dir.join(format!("thumbnail_{}.jpg", timestamp));
            println!("Thumbnail will be saved to: {:?}", thumbnail_file);

            // Use FFmpeg to extract thumbnail at 1 second
            if let Some(video_str) = video_path.to_str() {
                if let Some(thumb_str) = thumbnail_file.to_str() {
                    println!("Running FFmpeg command...");
                    #[cfg(target_os = "windows")]
                    let result = {
                        #[allow(unused_imports)]
                        use std::os::windows::process::CommandExt;
                        const CREATE_NO_WINDOW: u32 = 0x08000000;
                        Command::new("ffmpeg")
                            .args([
                                "-ss",
                                "00:00:01",
                                "-i",
                                video_str,
                                "-vframes",
                                "1",
                                "-q:v",
                                "2",
                                "-y",
                                thumb_str,
                            ])
                            .creation_flags(CREATE_NO_WINDOW)
                            .output()
                            .await
                    };
                    
                    #[cfg(not(target_os = "windows"))]
                    let result = Command::new("ffmpeg")
                        .args([
                            "-ss",
                            "00:00:01",
                            "-i",
                            video_str,
                            "-vframes",
                            "1",
                            "-q:v",
                            "2",
                            "-y",
                            thumb_str,
                        ])
                        .output()
                        .await;

                    match result {
                        Ok(output) => {
                            println!("FFmpeg exit status: {}", output.status);
                            if !output.status.success() {
                                println!("FFmpeg stderr: {}", String::from_utf8_lossy(&output.stderr));
                            }

                            if output.status.success() && thumbnail_file.exists() {
                                println!("Thumbnail extracted successfully!");
                                *thumbnail_path_arc.lock().unwrap() = Some(thumbnail_file);
                            } else {
                                println!("Thumbnail file does not exist or FFmpeg failed");
                            }
                        }
                        Err(e) => {
                            println!("Failed to run FFmpeg: {}", e);
                        }
                    }
                } else {
                    println!("Failed to convert thumbnail path to string");
                }
            } else {
                println!("Failed to convert video path to string");
            }
        });
    }

    fn convert(&self) {
        let input = self.input_path.clone().unwrap();
        let output = self.output_path.clone().unwrap();
        let status = Arc::clone(&self.status);

        *status.lock().unwrap() = Status::Converting;

        self.runtime.spawn(async move {
            #[cfg(target_os = "windows")]
            let result = {
                #[allow(unused_imports)]
                use std::os::windows::process::CommandExt;
                const CREATE_NO_WINDOW: u32 = 0x08000000;
                Command::new("ffmpeg")
                    .args([
                        "-i",
                        input.to_str().unwrap(),
                        "-vn",
                        "-acodec",
                        "libmp3lame",
                        "-ab",
                        "192k",
                        "-y",
                        output.to_str().unwrap(),
                    ])
                    .creation_flags(CREATE_NO_WINDOW)
                    .output()
                    .await
            };
            
            #[cfg(not(target_os = "windows"))]
            let result = Command::new("ffmpeg")
                .args([
                    "-i",
                    input.to_str().unwrap(),
                    "-vn",
                    "-acodec",
                    "libmp3lame",
                    "-ab",
                    "192k",
                    "-y",
                    output.to_str().unwrap(),
                ])
                .output()
                .await;

            let new_status = match result {
                Ok(out) if out.status.success() => Status::Done,
                Ok(out) => Status::Error(String::from_utf8_lossy(&out.stderr).to_string()),
                Err(e) => Status::Error(e.to_string()),
            };

            *status.lock().unwrap() = new_status;
        });
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Set dark mode
        ctx.set_visuals(egui::Visuals::dark());

        // Handle dropped files
        ctx.input(|i| {
            if !i.raw.dropped_files.is_empty() {
                if let Some(path) = i.raw.dropped_files[0].path.clone() {
                    self.set_input(path);
                    self.dropped_file = true;
                }
            }
        });

        // Show info popup window
        if self.show_info_popup {
            egui::Window::new("About")
                .collapsible(false)
                .resizable(false)
                .fixed_size(Vec2::new(250.0, 180.0))
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(10.0);
                        ui.label(
                            egui::RichText::new("Version 1.0")
                                .size(12.0)
                                .color(Color32::LIGHT_GRAY),
                        );
                        ui.add_space(5.0);
                        ui.label(
                            egui::RichText::new("Powered by FFmpeg")
                                .size(12.0)
                                .color(Color32::LIGHT_GRAY),
                        );
                        ui.add_space(15.0);
                        if ui.button("Close").clicked() {
                            self.show_info_popup = false;
                        }
                        ui.add_space(10.0);
                    });
                });
        }

        egui::CentralPanel::default()
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {

                    ui.add_space(20.0);

                    // Drop zone
                    let drop_zone_size = Vec2::new(250.0, 160.0);
                    let (rect, response) = ui.allocate_exact_size(drop_zone_size, egui::Sense::click());

                    // Draw dashed border
                    let painter = ui.painter();
                    let stroke = Stroke::new(2.0, Color32::GRAY);
                    let rounding = CornerRadius::same(12);

                    painter.rect_stroke(rect, rounding, stroke, egui::StrokeKind::Outside);

                    // Load and display thumbnail if video is selected
                    if self.input_path.is_some() {
                        // Check if thumbnail is ready to load
                        if self.video_thumbnail.is_none() {
                            let thumb_path_opt = self.thumbnail_path.lock().unwrap().clone();
                            if let Some(thumb_path) = thumb_path_opt {
                                println!("Loading thumbnail from: {:?}", thumb_path);
                                self.video_thumbnail = self.load_icon_from_file(ctx, thumb_path.to_str().unwrap());
                                if self.video_thumbnail.is_some() {
                                    println!("Thumbnail loaded successfully!");
                                } else {
                                    println!("Failed to load thumbnail image");
                                }
                                self.thumbnail_loading = false;
                            }
                        }

                        // Display thumbnail if available
                        if let Some(ref thumbnail) = self.video_thumbnail {
                            // Draw thumbnail inside the drop zone with rounded corners
                            let thumb_rect = rect.shrink(4.0); // Shrink slightly to fit within border

                            // Calculate rounded corners vertices and UVs
                            let uv_rect = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));

                            // Simple approach: draw image then mask with rounded rect
                            // Draw the image first
                            painter.image(
                                thumbnail.id(),
                                thumb_rect,
                                uv_rect,
                                Color32::WHITE,
                            );

                            // Draw a rounded rect frame to create the rounded corner effect
                            // by covering the corners with the background color
                            let bg_color = ui.visuals().window_fill();
                            painter.rect_stroke(thumb_rect, rounding, Stroke::new(4.0, bg_color), egui::StrokeKind::Outside);

                            // Optionally: Draw a subtle overlay on hover
                            if response.hovered() {
                                painter.rect_filled(thumb_rect, rounding, Color32::from_black_alpha(20));
                            }
                        } else {
                            // Draw play icon when thumbnail is loading or failed
                            let center = rect.center();
                            let icon_size = 40.0;

                            // Triangle play button
                            let points = vec![
                                egui::pos2(center.x - icon_size * 0.4, center.y - icon_size * 0.5),
                                egui::pos2(center.x - icon_size * 0.4, center.y + icon_size * 0.5),
                                egui::pos2(center.x + icon_size * 0.5, center.y),
                            ];
                            painter.add(egui::Shape::convex_polygon(
                                points,
                                Color32::GRAY,
                                Stroke::NONE,
                            ));

                            // Request repaint if still loading
                            if self.thumbnail_loading {
                                ctx.request_repaint();
                            }
                        }
                    } else {
                        // Draw play icon when no video selected
                        let center = rect.center();
                        let icon_size = 40.0;

                        // Triangle play button
                        let points = vec![
                            egui::pos2(center.x - icon_size * 0.4, center.y - icon_size * 0.5),
                            egui::pos2(center.x - icon_size * 0.4, center.y + icon_size * 0.5),
                            egui::pos2(center.x + icon_size * 0.5, center.y),
                        ];
                        painter.add(egui::Shape::convex_polygon(
                            points,
                            Color32::GRAY,
                            Stroke::NONE,
                        ));
                    }

                    // Change cursor to pointer hand on hover
                    if response.hovered() {
                        ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                    }

                    if response.clicked() {
                        if let Some(path) = FileDialog::new()
                            .add_filter("Video", &["mp4", "mkv", "avi", "mov", "webm", "flv"])
                            .pick_file()
                        {
                            self.set_input(path);
                        }
                    }

                    // Status text
                    let status = self.status.lock().unwrap().clone();
                    let text = if let Some(ref path) = self.input_path {
                        path.file_name().unwrap().to_string_lossy().to_string()
                    } else {
                        "Drop your video here to convert \n (\"mp4\", \"mkv\", \"avi\", \"mov\", \"webm\", \"flv\")".to_string()
                    };


                    let text_color = match &status {
                        Status::Done => Color32::from_rgb(74, 222, 128),
                        Status::Error(_) => Color32::from_rgb(248, 113, 113),
                        _ => Color32::LIGHT_GRAY,
                    };

                    let display_text = match &status {
                        Status::Converting => "Converting...".to_string(),
                        Status::Done => "Done!".to_string(),
                        Status::Error(_) => "Error occurred".to_string(),
                        _ => text,
                    };

                    ui.add_space(20.0);
                    // Status text with optional link icon (centered)
                    ui.vertical_centered(|ui| {
                    if matches!(status, Status::Done) {
                        // When done, use horizontal for text + icon
                        ui.horizontal(|ui| {
                            ui.add_space((ui.available_width() - 100.0) / 2.0); // Approximate centering
                            ui.label(
                                egui::RichText::new(&display_text)
                                    .size(11.0)
                                    .color(text_color),
                            );

                            if let Some(ref output_path) = self.output_path {
                                ui.add_space(5.0);
                                let link_btn = ui.add(
                                    egui::Button::new(egui::RichText::new("📂").size(14.0)).frame(false),
                                );

                                if link_btn.hovered() {
                                    ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                                }

                                if link_btn.clicked() {
                                    #[cfg(target_os = "windows")]
                                    {
                                        #[allow(unused_imports)]
                                        use std::os::windows::process::CommandExt;
                                        const CREATE_NO_WINDOW: u32 = 0x08000000;
                                        let _ = std::process::Command::new("explorer")
                                            .args(["/select,", output_path.to_str().unwrap()])
                                            .creation_flags(CREATE_NO_WINDOW)
                                            .spawn();
                                    }
                                    #[cfg(target_os = "macos")]
                                    {
                                        let _ = std::process::Command::new("open")
                                            .args(["-R", output_path.to_str().unwrap()])
                                            .spawn();
                                    }
                                    #[cfg(target_os = "linux")]
                                    {
                                        if let Some(parent) = output_path.parent() {
                                            let _ = std::process::Command::new("xdg-open").arg(parent).spawn();
                                        }
                                    }
                                }

                                link_btn.on_hover_text("Open file location");
                            }
                        });
                    } else {
                        // Simple centered label when not done
                        ui.label(
                            egui::RichText::new(&display_text)
                                .size(11.0)
                                .color(text_color),
                        );
                    }
});

                    ui.add_space(20.0);

                    // Bottom bar
                    ui.horizontal(|ui| {
                        ui.add_space(20.0);

                        // Convert button
                        let can_convert = self.input_path.is_some()
                            && !matches!(*self.status.lock().unwrap(), Status::Converting);

                        let btn_color = if can_convert {
                            Color32::from_rgb(34, 197, 94)
                        } else {
                            Color32::from_rgb(150, 200, 150)
                        };

                        let btn = ui.add_sized(
                            [250.0, 35.0],
                            egui::Button::new(
                                egui::RichText::new("Convert to MP3")
                                    .size(16.0)
                                    .color(Color32::WHITE),
                            )
                            .fill(btn_color)
                            .corner_radius(CornerRadius::same(25))
                        )
                        .on_hover_text("Start converting the selected video to MP3");

                        if btn.hovered() {
                            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                        }

                        if btn.clicked() && can_convert {
                            self.convert();
                        }
                        ui.add_space(20.0);
                    });
                    // ui.add_space(20.0);
                });
            });
    }
}