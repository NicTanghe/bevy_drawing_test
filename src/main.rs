mod paint;

use std::num::NonZeroU32;

use bevy::{
    input::{
        ButtonState,
        mouse::MouseButton,
        pen::{PenAction, PenButton, PenData, PenInput, PenPressure, PenToolKind},
    },
    math::Isometry2d,
    prelude::*,
    render::pipelined_rendering::PipelinedRenderingPlugin,
    window::{CursorLeft, CursorMoved, CursorOptions, PresentMode, PrimaryWindow, WindowPlugin},
    winit::WinitSettings,
};
use paint::{BrushSample, BrushShape, PaintCanvas, PaintOperation, Tool};

const START_WIDTH: u32 = 1_200;
const START_HEIGHT: u32 = 750;
const MIN_BRUSH_SIZE: f32 = 2.0;
const MAX_BRUSH_SIZE: f32 = 180.0;

fn main() {
    let default_plugins = DefaultPlugins
        .build()
        .set(WindowPlugin {
            primary_window: Some(Window {
                title: "Tilt Paint".into(),
                resolution: (START_WIDTH, START_HEIGHT).into(),
                present_mode: PresentMode::AutoNoVsync,
                desired_maximum_frame_latency: NonZeroU32::new(1),
                ..default()
            }),
            ..default()
        })
        .disable::<PipelinedRenderingPlugin>();

    App::new()
        .insert_resource(ClearColor(Color::srgb_u8(248, 247, 244)))
        .insert_resource(WinitSettings::continuous())
        .add_plugins(default_plugins)
        .init_resource::<BrushSettings>()
        .init_resource::<PaintInputState>()
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (
                toggle_vsync,
                collect_paint_input,
                fit_canvas_to_window,
                draw_brush_preview,
                update_hud,
            )
                .chain(),
        )
        .run();
}

#[derive(Resource)]
struct BrushSettings {
    pen_size: f32,
    eraser_size: f32,
}

impl Default for BrushSettings {
    fn default() -> Self {
        Self {
            pen_size: 18.0,
            eraser_size: 34.0,
        }
    }
}

impl BrushSettings {
    fn size(&self, tool: Tool) -> f32 {
        match tool {
            Tool::Pen => self.pen_size,
            Tool::Eraser => self.eraser_size,
        }
    }

    fn set_size(&mut self, tool: Tool, size: f32) {
        let destination = match tool {
            Tool::Pen => &mut self.pen_size,
            Tool::Eraser => &mut self.eraser_size,
        };
        *destination = size.clamp(MIN_BRUSH_SIZE, MAX_BRUSH_SIZE);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PointerSource {
    Mouse,
    Pen,
}

impl PointerSource {
    fn label(self) -> &'static str {
        match self {
            Self::Mouse => "mouse",
            Self::Pen => "tablet",
        }
    }
}

#[derive(Default)]
struct StrokeTracker {
    last: Option<BrushSample>,
}

#[derive(Clone, Copy)]
struct SizeGesture {
    source: PointerSource,
    tool: Tool,
    origin: Vec2,
    starting_size: f32,
}

#[derive(Resource)]
struct PaintInputState {
    mouse_position: Option<Vec2>,
    mouse_stroke: StrokeTracker,
    pen_stroke: StrokeTracker,
    pen_contact: bool,
    pen_barrel: bool,
    cursor: Option<BrushSample>,
    cursor_source: PointerSource,
    cursor_down: bool,
    active_tool: Tool,
    sizing: Option<SizeGesture>,
}

impl Default for PaintInputState {
    fn default() -> Self {
        Self {
            mouse_position: None,
            mouse_stroke: default(),
            pen_stroke: default(),
            pen_contact: false,
            pen_barrel: false,
            cursor: None,
            cursor_source: PointerSource::Mouse,
            cursor_down: false,
            active_tool: Tool::Pen,
            sizing: None,
        }
    }
}

impl PaintInputState {
    fn tracker_mut(&mut self, source: PointerSource) -> &mut StrokeTracker {
        match source {
            PointerSource::Mouse => &mut self.mouse_stroke,
            PointerSource::Pen => &mut self.pen_stroke,
        }
    }

    fn end_stroke(&mut self, source: PointerSource) {
        self.tracker_mut(source).last = None;
        if self.sizing.is_some_and(|gesture| gesture.source == source) {
            self.sizing = None;
        }
        if self.cursor_source == source {
            self.cursor_down = false;
        }
    }

    fn hover(&mut self, source: PointerSource, sample: BrushSample) {
        self.cursor = Some(sample);
        self.cursor_source = source;
        self.cursor_down = false;
        self.active_tool = sample.tool;
    }
}

#[derive(Component)]
struct CanvasTileSprite {
    origin: UVec2,
    size: UVec2,
}

#[derive(Component)]
struct HudText;

fn setup(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut cursor_options: Single<&mut CursorOptions, With<PrimaryWindow>>,
) {
    cursor_options.visible = false;
    commands.spawn(Camera2d);

    let canvas = PaintCanvas::new(&mut images);
    let window_size = Vec2::new(window.width(), window.height());
    for tile in canvas.tiles() {
        let tile_component = CanvasTileSprite {
            origin: tile.origin,
            size: tile.size,
        };
        let (display_size, position) =
            canvas_tile_layout(&tile_component, canvas.size().as_vec2(), window_size);
        let mut sprite = Sprite::from_image(tile.image.clone());
        sprite.custom_size = Some(display_size);
        commands.spawn((
            tile_component,
            sprite,
            Transform::from_xyz(position.x, position.y, 0.0),
        ));
    }
    commands.insert_resource(canvas);

    commands.spawn((
        HudText,
        Text::new("PEN"),
        TextFont::from_font_size(14.0),
        TextColor(Color::srgb(0.94, 0.95, 0.97)),
        Node {
            position_type: PositionType::Absolute,
            top: px(16),
            left: px(16),
            padding: UiRect::axes(px(13), px(9)),
            border_radius: BorderRadius::all(px(8)),
            ..default()
        },
        BackgroundColor(Color::srgba(0.055, 0.065, 0.085, 0.90)),
    ));
}

#[allow(clippy::too_many_arguments)]
fn collect_paint_input(
    mut pen_events: MessageReader<PenInput>,
    mut cursor_events: MessageReader<CursorMoved>,
    mut cursor_left_events: MessageReader<CursorLeft>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut canvas: ResMut<PaintCanvas>,
    mut images: ResMut<Assets<Image>>,
    mut settings: ResMut<BrushSettings>,
    mut input: ResMut<PaintInputState>,
) {
    let viewport_size = Vec2::new(window.width(), window.height());
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let mut operations = Vec::new();

    if !shift && let Some(gesture) = input.sizing.take() {
        input.tracker_mut(gesture.source).last = None;
    }

    for event in pen_events.read() {
        if !event.pen.primary {
            continue;
        }
        let tool = pen_tool(event.pen.tool, input.pen_barrel);
        match &event.action {
            PenAction::Entered => {
                if let Some(position) = event.pen.position {
                    input.hover(
                        PointerSource::Pen,
                        BrushSample {
                            position,
                            pressure: None,
                            tilt: Vec2::ZERO,
                            tool,
                        },
                    );
                }
            }
            PenAction::Moved(data) => {
                let Some(position) = event.pen.position else {
                    continue;
                };
                let sample = pen_sample(position, tool, data);
                let pressure_contact = sample.pressure.is_some_and(|value| value > 0.001);
                if input.pen_contact || pressure_contact {
                    handle_active_sample(
                        PointerSource::Pen,
                        sample,
                        shift,
                        viewport_size,
                        &mut input,
                        &mut settings,
                        &mut operations,
                    );
                } else {
                    input.end_stroke(PointerSource::Pen);
                    input.hover(PointerSource::Pen, sample);
                }
            }
            PenAction::Button {
                button,
                state,
                data,
            } => {
                if *button == PenButton::Barrel {
                    input.pen_barrel = state.is_pressed();
                    input.pen_stroke.last = None;
                }

                let tool = pen_tool(event.pen.tool, input.pen_barrel);
                let sample = event
                    .pen
                    .position
                    .map(|position| pen_sample(position, tool, data));
                if *button == PenButton::Contact {
                    match state {
                        ButtonState::Pressed => {
                            input.pen_contact = true;
                            if let Some(sample) = sample {
                                handle_active_sample(
                                    PointerSource::Pen,
                                    sample,
                                    shift,
                                    viewport_size,
                                    &mut input,
                                    &mut settings,
                                    &mut operations,
                                );
                            }
                        }
                        ButtonState::Released => {
                            input.pen_contact = false;
                            input.end_stroke(PointerSource::Pen);
                            if let Some(sample) = sample {
                                input.hover(PointerSource::Pen, sample);
                            }
                        }
                    }
                } else if let Some(sample) = sample {
                    if input.pen_contact {
                        handle_active_sample(
                            PointerSource::Pen,
                            sample,
                            shift,
                            viewport_size,
                            &mut input,
                            &mut settings,
                            &mut operations,
                        );
                    } else {
                        input.hover(PointerSource::Pen, sample);
                    }
                }
            }
            PenAction::Left => {
                input.pen_contact = false;
                input.pen_barrel = false;
                input.end_stroke(PointerSource::Pen);
                if input.cursor_source == PointerSource::Pen {
                    input.cursor = None;
                }
            }
        }
    }

    let mouse_down =
        mouse_buttons.pressed(MouseButton::Left) || mouse_buttons.pressed(MouseButton::Right);
    let mouse_tool = if mouse_buttons.pressed(MouseButton::Right) {
        Tool::Eraser
    } else {
        Tool::Pen
    };
    let mouse_started = mouse_buttons.just_pressed(MouseButton::Left)
        || mouse_buttons.just_pressed(MouseButton::Right);
    let mouse_ended = mouse_buttons.just_released(MouseButton::Left)
        || mouse_buttons.just_released(MouseButton::Right);
    if mouse_started {
        input.mouse_stroke.last = None;
    }

    let mut painted_mouse_move = false;
    for event in cursor_events.read() {
        input.mouse_position = Some(event.position);
        let sample = BrushSample {
            position: event.position,
            pressure: None,
            tilt: Vec2::ZERO,
            tool: if mouse_down {
                mouse_tool
            } else {
                input.active_tool
            },
        };
        if mouse_down {
            handle_active_sample(
                PointerSource::Mouse,
                sample,
                shift,
                viewport_size,
                &mut input,
                &mut settings,
                &mut operations,
            );
            painted_mouse_move = true;
        } else {
            input.hover(PointerSource::Mouse, sample);
        }
    }

    if mouse_started
        && mouse_down
        && !painted_mouse_move
        && let Some(position) = input.mouse_position
    {
        handle_active_sample(
            PointerSource::Mouse,
            BrushSample {
                position,
                pressure: None,
                tilt: Vec2::ZERO,
                tool: mouse_tool,
            },
            shift,
            viewport_size,
            &mut input,
            &mut settings,
            &mut operations,
        );
    }
    if mouse_ended || !mouse_down {
        input.end_stroke(PointerSource::Mouse);
    }
    for _ in cursor_left_events.read() {
        input.end_stroke(PointerSource::Mouse);
        if input.cursor_source == PointerSource::Mouse {
            input.cursor = None;
        }
    }

    if keys.just_pressed(KeyCode::KeyC) {
        operations.insert(0, PaintOperation::Clear);
        input.mouse_stroke.last = None;
        input.pen_stroke.last = None;
    }

    if operations.is_empty() {
        return;
    }
    for operation in operations {
        canvas.apply(operation);
    }
    canvas.upload_dirty(&mut images);
}

fn toggle_vsync(
    keys: Res<ButtonInput<KeyCode>>,
    mut window: Single<&mut Window, With<PrimaryWindow>>,
) {
    if keys.just_pressed(KeyCode::KeyV) {
        window.present_mode = match window.present_mode {
            PresentMode::AutoNoVsync | PresentMode::Immediate | PresentMode::Mailbox => {
                PresentMode::AutoVsync
            }
            _ => PresentMode::AutoNoVsync,
        };
        info!("presentation mode: {:?}", window.present_mode);
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_active_sample(
    source: PointerSource,
    sample: BrushSample,
    shift: bool,
    viewport_size: Vec2,
    input: &mut PaintInputState,
    settings: &mut BrushSettings,
    operations: &mut Vec<PaintOperation>,
) {
    input.cursor = Some(sample);
    input.cursor_source = source;
    input.cursor_down = true;
    input.active_tool = sample.tool;

    if shift {
        let start_new_gesture = input
            .sizing
            .is_none_or(|gesture| gesture.source != source || gesture.tool != sample.tool);
        if start_new_gesture {
            input.sizing = Some(SizeGesture {
                source,
                tool: sample.tool,
                origin: sample.position,
                starting_size: settings.size(sample.tool),
            });
            input.tracker_mut(source).last = None;
        }

        if let Some(gesture) = input.sizing {
            // Dragging right or upward grows the active tool; the reverse shrinks it.
            let delta =
                (sample.position.x - gesture.origin.x) - (sample.position.y - gesture.origin.y);
            settings.set_size(gesture.tool, gesture.starting_size + delta * 0.35);
        }
        return;
    }

    if input.sizing.is_some_and(|gesture| gesture.source == source) {
        input.sizing = None;
        input.tracker_mut(source).last = None;
    }
    let last = input
        .tracker_mut(source)
        .last
        .filter(|last| last.tool == sample.tool);
    let nominal_diameter = settings.size(sample.tool);
    if let Some(last) = last {
        let spacing = BrushShape::from_sample(last, nominal_diameter)
            .dab_spacing()
            .min(BrushShape::from_sample(sample, nominal_diameter).dab_spacing());
        if last.position.distance(sample.position) < spacing {
            return;
        }
    }
    operations.push(PaintOperation::Stroke {
        from: last,
        to: sample,
        nominal_diameter,
        viewport_size,
    });
    input.tracker_mut(source).last = Some(sample);
}

fn pen_tool(kind: PenToolKind, barrel_pressed: bool) -> Tool {
    if kind == PenToolKind::Eraser || barrel_pressed {
        Tool::Eraser
    } else {
        Tool::Pen
    }
}

fn pen_sample(position: Vec2, tool: Tool, data: &PenData) -> BrushSample {
    BrushSample {
        position,
        pressure: data.pressure.map(normalize_pressure),
        tilt: pen_tilt(data),
        tool,
    }
}

fn normalize_pressure(pressure: PenPressure) -> f32 {
    match pressure {
        PenPressure::Normalized(value) => value as f32,
        PenPressure::Calibrated {
            force,
            max_possible_force,
        } if max_possible_force > f64::EPSILON => (force / max_possible_force) as f32,
        PenPressure::Calibrated { .. } => 0.0,
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

fn fit_canvas_to_window(
    window: Single<&Window, With<PrimaryWindow>>,
    canvas: Res<PaintCanvas>,
    mut tiles: Query<(&CanvasTileSprite, &mut Sprite, &mut Transform)>,
) {
    let window_size = Vec2::new(window.width(), window.height());
    let canvas_size = canvas.size().as_vec2();
    for (tile, mut sprite, mut transform) in &mut tiles {
        let (display_size, position) = canvas_tile_layout(tile, canvas_size, window_size);
        if sprite.custom_size != Some(display_size) {
            sprite.custom_size = Some(display_size);
        }
        if transform.translation.xy() != position {
            transform.translation = position.extend(0.0);
        }
    }
}

fn canvas_tile_layout(
    tile: &CanvasTileSprite,
    canvas_size: Vec2,
    window_size: Vec2,
) -> (Vec2, Vec2) {
    let display_size = tile.size.as_vec2() / canvas_size * window_size;
    let center = (tile.origin.as_vec2() + tile.size.as_vec2() * 0.5) / canvas_size;
    let position = Vec2::new(
        (center.x - 0.5) * window_size.x,
        (0.5 - center.y) * window_size.y,
    );
    (display_size, position)
}

fn draw_brush_preview(
    mut gizmos: Gizmos,
    window: Single<&Window, With<PrimaryWindow>>,
    input: Res<PaintInputState>,
    settings: Res<BrushSettings>,
) {
    let Some(mut sample) = input.cursor else {
        return;
    };
    // Hover shows the nominal footprint; contact shows live pressure.
    if !input.cursor_down {
        sample.pressure = None;
    }
    let shape = BrushShape::from_sample(sample, settings.size(sample.tool));
    let world_position = Vec2::new(
        sample.position.x - window.width() * 0.5,
        window.height() * 0.5 - sample.position.y,
    );
    let color = match sample.tool {
        Tool::Pen => Color::srgb(0.10, 0.48, 0.95),
        Tool::Eraser => Color::srgb(0.94, 0.25, 0.34),
    };
    gizmos
        .ellipse_2d(
            Isometry2d::new(world_position, Rot2::radians(-shape.angle)),
            shape.half_size,
            color,
        )
        .resolution(48);
}

fn update_hud(
    input: Res<PaintInputState>,
    settings: Res<BrushSettings>,
    mut text: Single<&mut Text, With<HudText>>,
) {
    let pressure = input.cursor.and_then(|sample| sample.pressure).map_or_else(
        || "full".to_string(),
        |value| format!("{:.0}%", value * 100.0),
    );
    let tilt = input
        .cursor
        .map_or(0.0, |sample| sample.tilt.length().clamp(0.0, 90.0));
    let mode = if input.sizing.is_some() {
        "  •  SIZING"
    } else {
        ""
    };
    let content = format!(
        "{}  •  {:.0} px  •  pressure {}  •  tilt {:.0}°  •  {}{}\n\
         LMB pen   RMB / barrel / eraser tip erase   Shift + drag sizes   C clears   V vsync",
        input.active_tool.label(),
        settings.size(input.active_tool),
        pressure,
        tilt,
        input.cursor_source.label(),
        mode,
    );
    if text.0 != content {
        text.0 = content;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calibrated_pressure_is_normalized_and_clamped() {
        assert_eq!(
            normalize_pressure(PenPressure::Calibrated {
                force: 256.0,
                max_possible_force: 1024.0,
            }),
            0.25
        );
        assert_eq!(normalize_pressure(PenPressure::Normalized(4.0)), 1.0);
    }

    #[test]
    fn physical_eraser_and_barrel_select_the_eraser() {
        assert_eq!(pen_tool(PenToolKind::Eraser, false), Tool::Eraser);
        assert_eq!(pen_tool(PenToolKind::Pen, true), Tool::Eraser);
        assert_eq!(pen_tool(PenToolKind::Pen, false), Tool::Pen);
    }

    #[test]
    fn sub_spacing_input_does_not_repaint_the_same_dab() {
        let mut input = PaintInputState::default();
        let mut settings = BrushSettings::default();
        let mut operations = Vec::new();
        let viewport = Vec2::new(1_200.0, 750.0);
        let first = BrushSample {
            position: Vec2::new(100.0, 100.0),
            pressure: Some(0.7),
            tilt: Vec2::ZERO,
            tool: Tool::Pen,
        };

        handle_active_sample(
            PointerSource::Pen,
            first,
            false,
            viewport,
            &mut input,
            &mut settings,
            &mut operations,
        );
        handle_active_sample(
            PointerSource::Pen,
            BrushSample {
                position: first.position + Vec2::splat(0.1),
                pressure: Some(0.72),
                ..first
            },
            false,
            viewport,
            &mut input,
            &mut settings,
            &mut operations,
        );
        assert_eq!(operations.len(), 1);

        handle_active_sample(
            PointerSource::Pen,
            BrushSample {
                position: first.position + Vec2::new(8.0, 0.0),
                ..first
            },
            false,
            viewport,
            &mut input,
            &mut settings,
            &mut operations,
        );
        assert_eq!(operations.len(), 2);
    }
}
