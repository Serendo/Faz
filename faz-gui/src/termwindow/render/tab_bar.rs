use crate::quad::TripleLayerQuadAllocator;
use crate::tabbar::TabBarItem;
use crate::termwindow::render::{forces_opaque_faz_tui_window_background, RenderScreenLineParams};
use crate::termwindow::{UIItem, UIItemType};
use crate::utilsprites::RenderMetrics;
use config::ConfigHandle;
use mux::renderable::RenderableDimensions;
use wezterm_term::color::ColorAttribute;
use window::color::LinearRgba;

impl crate::TermWindow {
    pub fn paint_tab_bar(&mut self, layers: &mut TripleLayerQuadAllocator) -> anyhow::Result<()> {
        let tab_bar_height = self.tab_bar_pixel_height()?;
        // The bar keeps its one line of cells; the strip above it stays empty
        // and draggable. Its bottom edge lands on the strip line derived from
        // the window buttons, and the terminal content starts there too.
        // The bar hangs from the strip line: its top is strip minus its own
        // height, so its bottom edge is the line itself. Fullscreen has no
        // buttons and no strip, so the bar sits at the very top.
        let tab_bar_y = if self.layout_is_effective_fullscreen() {
            0.
        } else {
            let strip = self.title_bar_strip_height();
            (strip - tab_bar_height).max(0.)
        };
        let panes = self.get_panes_to_render();
        let force_opaque_tab_bar_background = forces_opaque_faz_tui_window_background(&panes);

        let palette = self.palette().clone();

        // Top tabs sit under the macOS titlebar mask; honor line_height so tall
        // fonts don't clip against the top edge.
        let tab_metrics = self.render_metrics;

        // The strip the tab bar leaves above itself — the space the standard
        // window buttons sit in — is window chrome, not terminal content. Mark
        // it as a drag target so clicking and dragging there moves the window,
        // the way a titlebar does. Pushed *before* the tab entries because the
        // hit test walks the list in reverse, so real tabs still win.
        if tab_bar_y > 0. {
            self.ui_items.push(UIItem {
                x: 0,
                width: self.dimensions.pixel_width,
                y: 0,
                height: tab_bar_y as usize,
                item_type: UIItemType::TabBar(TabBarItem::None),
            });
        }

        self.ui_items.append(&mut self.tab_bar.compute_ui_items(
            tab_bar_y as usize,
            tab_metrics.cell_size.height as usize,
            tab_metrics.cell_size.width as usize,
        ));

        let window_is_transparent =
            !self.window_background.is_empty() || self.config.window_background_opacity != 1.0;
        let effective_window_is_transparent =
            window_is_transparent && !force_opaque_tab_bar_background;
        let gl_state = self.render_state.as_ref().unwrap();
        let white_space = gl_state.util_sprites.white_space.texture_coords();
        let filled_box = gl_state.util_sprites.filled_box.texture_coords();
        let default_bg = palette
            .resolve_bg(ColorAttribute::Default)
            .to_linear()
            .mul_alpha(if effective_window_is_transparent {
                0.
            } else {
                self.config.text_background_opacity
            });

        self.fill_tab_bar_background(layers, tab_bar_y, tab_bar_height)?;

        self.render_screen_line(
            RenderScreenLineParams {
                top_pixel_y: tab_bar_y,
                left_pixel_x: 0.,
                pixel_width: self.dimensions.pixel_width as f32,
                stable_line_idx: None,
                line: self.tab_bar.line(),
                selection: 0..0,
                cursor: &Default::default(),
                palette: &palette,
                dims: &RenderableDimensions {
                    cols: self.dimensions.pixel_width / tab_metrics.cell_size.width as usize,
                    physical_top: 0,
                    scrollback_rows: 0,
                    scrollback_top: 0,
                    viewport_rows: 1,
                    dpi: self.terminal_size.dpi,
                    pixel_height: tab_metrics.cell_size.height as usize,
                    pixel_width: self.terminal_size.pixel_width,
                    reverse_video: false,
                },
                config: &self.config,
                cursor_border_color: LinearRgba::default(),
                foreground: palette.foreground.to_linear(),
                pane: None,
                is_active: true,
                selection_fg: LinearRgba::default(),
                selection_bg: LinearRgba::default(),
                cursor_fg: LinearRgba::default(),
                cursor_bg: LinearRgba::default(),
                cursor_is_default_color: true,
                white_space,
                filled_box,
                window_is_transparent: effective_window_is_transparent,
                default_bg,
                style: None,
                font: None,
                use_pixel_positioning: self.config.experimental_pixel_positioning,
                render_metrics: tab_metrics,
                shape_key: None,
                password_input: false,
            },
            layers,
        )?;

        Ok(())
    }

    /// Paint the window background behind the tab-bar strip.
    ///
    /// Nothing else fills this region (see the `top_fill_height` note in
    /// paint.rs), so without this the strip shows the window backing, which is
    /// a second translucent layer and reads lighter than the terminal body.
    /// Called both when the bar is visible and when it is hidden for a single
    /// tab, so the strip keeps the same colour either way.
    pub(crate) fn fill_tab_bar_background(
        &self,
        layers: &mut TripleLayerQuadAllocator,
        tab_bar_y: f32,
        tab_bar_height: f32,
    ) -> anyhow::Result<()> {
        let window_is_transparent =
            !self.window_background.is_empty() || self.config.window_background_opacity != 1.0;
        if !window_is_transparent {
            return Ok(());
        }
        let pane_background = self
            .get_active_pane_or_overlay()
            .map(|pane| pane.palette().background.to_linear())
            .unwrap_or_else(|| {
                self.config
                    .resolved_palette
                    .background
                    .unwrap_or_default()
                    .to_linear()
            });
        let bg = pane_background.mul_alpha(self.config.window_background_opacity);
        let top = 0.0;
        self.filled_rectangle(
            layers,
            0,
            euclid::rect(
                0.0,
                top,
                self.dimensions.pixel_width as f32,
                tab_bar_y + tab_bar_height - top,
            ),
            bg,
        )?;
        Ok(())
    }

    pub fn tab_bar_pixel_height_impl(
        config: &ConfigHandle,
        fontconfig: &wezterm_font::FontConfiguration,
        render_metrics: &RenderMetrics,
    ) -> anyhow::Result<f32> {
        let _ = fontconfig;
        let _ = config;
        Ok(render_metrics.cell_size.height as f32)
    }

    /// Cheap approximation of tab bar height that avoids the ~485ms cost of
    /// resolving the title font on macOS cold start (CoreText substitution
    /// lookup + HarfBuzz shaper init). Used only to compute initial window
    /// dimensions; the real height is computed lazily on first render via
    /// `tab_bar_pixel_height()`.
    pub fn estimated_tab_bar_pixel_height(
        config: &ConfigHandle,
        render_metrics: &RenderMetrics,
    ) -> f32 {
        let _ = config;
        render_metrics.cell_size.height as f32
    }

    pub fn tab_bar_pixel_height(&self) -> anyhow::Result<f32> {
        Self::tab_bar_pixel_height_impl(&self.config, &self.fonts, &self.render_metrics)
    }

    /// Distance from the window top to the first row of terminal content, for
    /// a top-positioned tab bar.
    ///
    /// The strip that holds the window buttons is chrome: the tab bar's one
    /// line sits at its bottom, and the content starts right below it.
    pub(crate) fn top_chrome_height(&self) -> f32 {
        // Fullscreen has no titlebar and no window buttons, so there is no
        // strip to clear — but a visible tab bar still needs its own height,
        // otherwise the terminal rows run under it.
        if self.layout_is_effective_fullscreen() {
            if !self.show_tab_bar {
                return 0.;
            }
            return self.tab_bar_pixel_height().unwrap_or(0.);
        }
        // No bar, no strip: with the tab row hidden (a single tab) the terminal
        // takes the space back instead of leaving an empty band under the
        // titlebar.
        if !self.show_tab_bar {
            return 0.;
        }
        self.title_bar_strip_height()
            .max(self.tab_bar_pixel_height().unwrap_or(0.))
    }

    /// Height of the strip that holds the standard window buttons, in device
    /// pixels. Zero when the platform did not report it.
    ///
    /// AppKit reports the button frame in points while this layer works in
    /// device pixels, hence the dpi conversion.
    pub(crate) fn title_bar_strip_height(&self) -> f32 {
        let dpi_scale = self.dimensions.dpi as f32 / 96.0;
        self.os_parameters
            .as_ref()
            .and_then(|p| p.title_bar.height)
            .map(|h| h.get() as f32 * dpi_scale)
            .unwrap_or(0.)
    }
}
