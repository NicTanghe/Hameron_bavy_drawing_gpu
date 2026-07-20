use std::num::NonZeroU32;

use bevy::{
    input::{
        mouse::MouseButton,
        pen::{PenAction, PenButton, PenData, PenInput, PenPressure, PenToolKind},
    },
    prelude::*,
    render::pipelined_rendering::PipelinedRenderingPlugin,
    window::{CursorLeft, CursorMoved, CursorOptions, PresentMode, PrimaryWindow, WindowPlugin},
    winit::WinitSettings,
};
use hamerons_stroke_render::{
    BrushProfile, BrushSizeSpace, CanvasTileCache, HameronsStrokeRenderPlugin, StrokeDocument,
    StrokeId, StrokePoint, StrokeRendererSettings,
};

const START_WIDTH: u32 = 1_200;
const START_HEIGHT: u32 = 750;
const MIN_BRUSH_SIZE: f32 = 2.0;
const MAX_BRUSH_SIZE: f32 = 180.0;

fn main() {
    let default_plugins = DefaultPlugins
        .build()
        .set(WindowPlugin {
            primary_window: Some(Window {
                title: "Hamerons Stroke Lab".into(),
                resolution: (START_WIDTH, START_HEIGHT).into(),
                present_mode: PresentMode::AutoNoVsync,
                desired_maximum_frame_latency: NonZeroU32::new(1),
                ..default()
            }),
            ..default()
        })
        .disable::<PipelinedRenderingPlugin>();

    let mut renderer_settings = StrokeRendererSettings::default();
    renderer_settings.pen.diameter = 18.0;
    renderer_settings.eraser.diameter = 34.0;
    renderer_settings.log_diagnostics = false;

    App::new()
        .insert_resource(ClearColor(Color::srgb_u8(248, 247, 244)))
        .insert_resource(WinitSettings::continuous())
        .insert_resource(renderer_settings)
        .add_plugins(default_plugins)
        .add_plugins(HameronsStrokeRenderPlugin)
        .init_resource::<PointerState>()
        .init_resource::<MouseStroke>()
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (
                collect_mouse_strokes,
                observe_pen_pointer,
                keyboard_shortcuts,
                draw_brush_preview,
                update_hud,
            )
                .chain(),
        )
        .run();
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Tool {
    #[default]
    Pen,
    Eraser,
}

impl Tool {
    fn label(self) -> &'static str {
        match self {
            Self::Pen => "PEN",
            Self::Eraser => "ERASER",
        }
    }

    fn profile(self, settings: &StrokeRendererSettings) -> BrushProfile {
        match self {
            Self::Pen => settings.pen,
            Self::Eraser => settings.eraser,
        }
    }

    fn profile_mut(self, settings: &mut StrokeRendererSettings) -> &mut BrushProfile {
        match self {
            Self::Pen => &mut settings.pen,
            Self::Eraser => &mut settings.eraser,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum PointerSource {
    #[default]
    Mouse,
    Tablet,
}

impl PointerSource {
    fn label(self) -> &'static str {
        match self {
            Self::Mouse => "mouse",
            Self::Tablet => "tablet",
        }
    }
}

#[derive(Resource, Default)]
struct PointerState {
    position: Option<Vec2>,
    pressure: Option<f32>,
    tilt: Vec2,
    tool: Tool,
    source: PointerSource,
    down: bool,
    pen_contact: bool,
}

impl PointerState {
    fn show_mouse(&mut self, position: Vec2, tool: Tool, down: bool) {
        self.position = Some(position);
        self.pressure = None;
        self.tilt = Vec2::ZERO;
        self.tool = tool;
        self.source = PointerSource::Mouse;
        self.down = down;
    }

    fn show_pen(&mut self, position: Vec2, tool: Tool, data: Option<&PenData>) {
        self.position = Some(position);
        self.pressure = data.and_then(|data| data.pressure).map(normalize_pressure);
        self.tilt = data.map_or(Vec2::ZERO, pen_tilt);
        self.tool = tool;
        self.source = PointerSource::Tablet;
        self.down = self.pen_contact;
    }
}

#[derive(Resource, Default)]
struct MouseStroke {
    stroke: Option<StrokeId>,
    tool: Tool,
    last_position: Option<Vec2>,
}

#[derive(Component)]
struct HudText;

fn setup(
    mut commands: Commands,
    mut cursor_options: Single<&mut CursorOptions, With<PrimaryWindow>>,
) {
    cursor_options.visible = false;
    commands.spawn(Camera2d);

    commands.spawn((
        HudText,
        Text::new("HAMERONS STROKE  /  READY"),
        TextFont::from_font_size(14.0),
        TextColor(Color::srgb(0.94, 0.95, 0.97)),
        Node {
            position_type: PositionType::Absolute,
            top: px(16),
            left: px(16),
            padding: UiRect::axes(px(13), px(9)),
            border: UiRect::all(px(1)),
            border_radius: BorderRadius::all(px(8)),
            ..default()
        },
        BorderColor::all(Color::srgba(0.35, 0.55, 0.85, 0.35)),
        BackgroundColor(Color::srgba(0.055, 0.065, 0.085, 0.92)),
    ));
}

#[allow(clippy::too_many_arguments)]
fn collect_mouse_strokes(
    mut cursor_events: MessageReader<CursorMoved>,
    mut cursor_left_events: MessageReader<CursorLeft>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform), With<Camera2d>>,
    settings: Res<StrokeRendererSettings>,
    mut document: ResMut<StrokeDocument>,
    mut mouse: ResMut<MouseStroke>,
    mut pointer: ResMut<PointerState>,
) {
    let mut latest_position = pointer
        .position
        .filter(|_| pointer.source == PointerSource::Mouse);
    for event in cursor_events.read() {
        latest_position = Some(event.position);
    }

    let down =
        mouse_buttons.pressed(MouseButton::Left) || mouse_buttons.pressed(MouseButton::Right);
    let tool = if mouse_buttons.pressed(MouseButton::Right) {
        Tool::Eraser
    } else if down {
        Tool::Pen
    } else {
        pointer.tool
    };
    let pressed = mouse_buttons.just_pressed(MouseButton::Left)
        || mouse_buttons.just_pressed(MouseButton::Right);

    if down && (pressed || mouse.tool != tool) {
        end_mouse_stroke(&mut document, &mut mouse);
    }

    if let Some(position) = latest_position {
        pointer.show_mouse(position, tool, down);

        if down {
            let (camera, camera_transform) = *camera;
            if let Some(point) = mouse_point(
                position,
                tool.profile(&settings),
                camera,
                camera_transform,
                &window,
            ) {
                if let Some(stroke) = mouse.stroke {
                    if mouse.last_position != Some(position) {
                        document.append_point(stroke, point);
                    }
                } else {
                    let (stroke, _) = document.begin_stroke(point, tool.profile(&settings));
                    mouse.stroke = Some(stroke);
                    mouse.tool = tool;
                }
                mouse.last_position = Some(position);
            }
        }
    }

    if !down
        || mouse_buttons.just_released(MouseButton::Left)
        || mouse_buttons.just_released(MouseButton::Right)
    {
        end_mouse_stroke(&mut document, &mut mouse);
        if pointer.source == PointerSource::Mouse {
            pointer.down = false;
        }
    }

    for _ in cursor_left_events.read() {
        end_mouse_stroke(&mut document, &mut mouse);
        if pointer.source == PointerSource::Mouse {
            pointer.position = None;
            pointer.down = false;
        }
    }
}

fn end_mouse_stroke(document: &mut StrokeDocument, mouse: &mut MouseStroke) {
    if let Some(stroke) = mouse.stroke.take() {
        document.end_stroke(stroke);
    }
    mouse.last_position = None;
}

fn mouse_point(
    viewport_position: Vec2,
    profile: BrushProfile,
    camera: &Camera,
    camera_transform: &GlobalTransform,
    window: &Window,
) -> Option<StrokePoint> {
    let position = camera
        .viewport_to_world_2d(camera_transform, viewport_position)
        .ok()?;
    let neighbor = camera
        .viewport_to_world_2d(camera_transform, viewport_position + Vec2::X)
        .or_else(|_| camera.viewport_to_world_2d(camera_transform, viewport_position - Vec2::X))
        .ok()?;
    let half_width = match profile.size_space {
        BrushSizeSpace::Document => brush_radius(profile, 1.0),
        BrushSizeSpace::Screen => {
            brush_radius(profile, 1.0) * position.distance(neighbor)
                / window.scale_factor().max(0.01)
        }
    };

    Some(StrokePoint {
        position,
        half_width,
        flow: profile.flow.clamp(0.0, 1.0),
        orientation: Vec2::Y,
        twist_radians: 0.0,
    })
}

fn observe_pen_pointer(mut pen_events: MessageReader<PenInput>, mut pointer: ResMut<PointerState>) {
    for event in pen_events.read() {
        if !event.pen.primary {
            continue;
        }
        let tool = if event.pen.tool == PenToolKind::Eraser {
            Tool::Eraser
        } else {
            Tool::Pen
        };

        match &event.action {
            PenAction::Entered => {
                if let Some(position) = event.pen.position {
                    pointer.show_pen(position, tool, None);
                }
            }
            PenAction::Moved(data) => {
                if let Some(position) = event.pen.position {
                    pointer.show_pen(position, tool, Some(data));
                }
            }
            PenAction::Button {
                button: PenButton::Contact,
                state,
                data,
            } => {
                pointer.pen_contact = state.is_pressed();
                if let Some(position) = event.pen.position {
                    pointer.show_pen(position, tool, Some(data));
                }
            }
            PenAction::Left => {
                pointer.pen_contact = false;
                if pointer.source == PointerSource::Tablet {
                    pointer.position = None;
                    pointer.down = false;
                }
            }
            PenAction::Button { .. } => {}
        }
    }
}

fn keyboard_shortcuts(
    keys: Res<ButtonInput<KeyCode>>,
    mut window: Single<&mut Window, With<PrimaryWindow>>,
    mut settings: ResMut<StrokeRendererSettings>,
    mut document: ResMut<StrokeDocument>,
    mut mouse: ResMut<MouseStroke>,
    pointer: Res<PointerState>,
) {
    let ctrl = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);

    if keys.just_pressed(KeyCode::KeyC) {
        document.clear();
        mouse.stroke = None;
        mouse.last_position = None;
    }
    if ctrl && keys.just_pressed(KeyCode::KeyZ) {
        if shift {
            document.redo();
        } else {
            document.undo();
        }
    }
    if ctrl && keys.just_pressed(KeyCode::KeyY) {
        document.redo();
    }

    let size_delta = if keys.just_pressed(KeyCode::BracketLeft) {
        -2.0
    } else if keys.just_pressed(KeyCode::BracketRight) {
        2.0
    } else {
        0.0
    };
    if size_delta != 0.0 {
        let profile = pointer.tool.profile_mut(&mut settings);
        profile.diameter = (profile.diameter + size_delta).clamp(MIN_BRUSH_SIZE, MAX_BRUSH_SIZE);
    }

    if keys.just_pressed(KeyCode::KeyV) {
        window.present_mode = match window.present_mode {
            PresentMode::AutoNoVsync | PresentMode::Immediate | PresentMode::Mailbox => {
                PresentMode::AutoVsync
            }
            _ => PresentMode::AutoNoVsync,
        };
    }
}

fn draw_brush_preview(
    mut gizmos: Gizmos,
    window: Single<&Window, With<PrimaryWindow>>,
    pointer: Res<PointerState>,
    settings: Res<StrokeRendererSettings>,
) {
    let Some(position) = pointer.position else {
        return;
    };
    let pressure = if pointer.down {
        pointer.pressure.unwrap_or(1.0)
    } else {
        1.0
    };
    let radius =
        brush_radius(pointer.tool.profile(&settings), pressure) / window.scale_factor().max(0.01);
    let world_position = Vec2::new(
        position.x - window.width() * 0.5,
        window.height() * 0.5 - position.y,
    );
    let color = match pointer.tool {
        Tool::Pen => Color::srgb(0.10, 0.48, 0.95),
        Tool::Eraser => Color::srgb(0.94, 0.25, 0.34),
    };

    gizmos
        .circle_2d(world_position, radius.max(0.65), color)
        .resolution(48);
    if pointer.tilt.length_squared() > 0.01 {
        let direction = Vec2::new(pointer.tilt.x, -pointer.tilt.y).normalize_or_zero();
        gizmos.line_2d(
            world_position,
            world_position + direction * radius.max(7.0),
            color,
        );
    }
}

fn update_hud(
    pointer: Res<PointerState>,
    settings: Res<StrokeRendererSettings>,
    document: Res<StrokeDocument>,
    tiles: Res<CanvasTileCache>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut text: Single<&mut Text, With<HudText>>,
) {
    let pressure = pointer.pressure.map_or_else(
        || "full".to_string(),
        |value| format!("{:.0}%", value * 100.0),
    );
    let tilt = pointer.tilt.length().clamp(0.0, 90.0);
    let profile = pointer.tool.profile(&settings);
    let cache = tiles.stats();
    let presentation = match window.present_mode {
        PresentMode::AutoNoVsync | PresentMode::Immediate | PresentMode::Mailbox => "low latency",
        _ => "vsync",
    };
    let content = format!(
        "HAMERONS STROKE  /  {}  •  {:.0} px  •  pressure {}  •  tilt {:.0}°  •  {}\n\
         LMB draw   RMB erase   tablet pressure + eraser tip   [ ] size   C clear   Ctrl+Z undo   V {}\n\
         ENGINE  •  {} strokes  •  {} points  •  {} segments  •  {} resident tiles",
        pointer.tool.label(),
        profile.diameter,
        pressure,
        tilt,
        pointer.source.label(),
        presentation,
        document.strokes().len(),
        document.points().len(),
        document.segments().len(),
        cache.resident_tiles,
    );
    if text.0 != content {
        text.0 = content;
    }
}

fn brush_radius(profile: BrushProfile, pressure: f32) -> f32 {
    let pressure = pressure
        .clamp(0.0, 1.0)
        .powf(profile.pressure_gamma.max(0.01));
    let minimum = profile.minimum_diameter_ratio.clamp(0.0, 1.0);
    let diameter = profile.diameter.max(0.25) * (minimum + (1.0 - minimum) * pressure);
    diameter * 0.5
}

fn normalize_pressure(pressure: PenPressure) -> f32 {
    match pressure {
        PenPressure::Normalized(value) if value.is_finite() => value as f32,
        PenPressure::Calibrated {
            force,
            max_possible_force,
        } if force.is_finite()
            && max_possible_force.is_finite()
            && max_possible_force > f64::EPSILON =>
        {
            (force / max_possible_force) as f32
        }
        _ => 1.0,
    }
    .clamp(0.0, 1.0)
}

fn pen_tilt(data: &PenData) -> Vec2 {
    if let Some(tilt) = data.tilt {
        return Vec2::new(tilt.x as f32, tilt.y as f32);
    }
    if let Some(angle) = data.angle {
        let magnitude = (std::f64::consts::FRAC_PI_2 - angle.altitude)
            .to_degrees()
            .clamp(0.0, 90.0) as f32;
        return Vec2::from_angle(angle.azimuth as f32) * magnitude;
    }
    Vec2::ZERO
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_radius_matches_the_engine_pressure_curve() {
        let profile = StrokeRendererSettings::default().pen;
        assert_eq!(brush_radius(profile, 1.0), profile.diameter * 0.5);
        assert!(brush_radius(profile, 0.0) >= profile.diameter * 0.05);
        assert!(brush_radius(profile, 0.0) < brush_radius(profile, 0.5));
    }

    #[test]
    fn pressure_is_normalized_and_clamped() {
        assert_eq!(normalize_pressure(PenPressure::Normalized(4.0)), 1.0);
        assert_eq!(
            normalize_pressure(PenPressure::Calibrated {
                force: 256.0,
                max_possible_force: 1024.0,
            }),
            0.25
        );
    }
}
