//! One native view as the shell sees it: the app logic plus, on Apple targets, the GPU
//! surface. The C ABI in `ffi` is a thin wrapper over this type.

use std::time::Duration;

use crate::app::App;
use crate::geometry::Viewport;
use crate::input::Phase;

/// Why a view call failed.
#[derive(Debug)]
pub enum ViewError {
    /// The core could not be built.
    Core(krypa_core::CoreError),
    /// The GPU could not be set up or drawing failed for a reason that retrying will not fix.
    Gpu(String),
    /// This build has no GPU backend (anything that is not an Apple target).
    Unsupported,
}

/// What the shell should do after a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    /// Draw again at the next display-link tick (an animation runs).
    Immediately,
    /// Draw again after this long, or earlier on input.
    After(Duration),
    /// Nothing changes until the next touch or resize: the display link may sleep.
    Idle,
}

/// App logic and, once attached, the drawing surface.
pub struct View {
    app: App,
    #[cfg(target_vendor = "apple")]
    gpu: Option<crate::gpu::Gpu>,
}

impl View {
    /// A view with no GPU: it runs egui and discards the output. Used by tests and by hosts
    /// that have no Metal.
    ///
    /// # Errors
    /// [`ViewError::Core`] if the core cannot start.
    pub fn headless(viewport: Viewport) -> Result<Self, ViewError> {
        Ok(Self {
            app: App::new(viewport).map_err(ViewError::Core)?,
            #[cfg(target_vendor = "apple")]
            gpu: None,
        })
    }

    /// A view that draws into the `CAMetalLayer` at `layer`.
    ///
    /// # Safety
    /// `layer` must point to a live `CAMetalLayer` that stays alive, and is not drawn to by
    /// anything else, until this view is dropped.
    #[cfg(target_vendor = "apple")]
    #[allow(unsafe_code)]
    pub unsafe fn with_layer(
        layer: *mut std::ffi::c_void,
        viewport: Viewport,
    ) -> Result<Self, ViewError> {
        let mut view = Self::headless(viewport)?;
        // SAFETY: forwarded unchanged from this function's contract.
        view.gpu = Some(unsafe { crate::gpu::Gpu::new(layer, &viewport) }.map_err(ViewError::Gpu)?);
        Ok(view)
    }

    /// Size, scale or insets changed.
    pub fn resize(&mut self, viewport: Viewport) {
        self.app.set_viewport(viewport);
        #[cfg(target_vendor = "apple")]
        if let Some(gpu) = &mut self.gpu {
            gpu.resize(&viewport);
        }
    }

    /// Queues a touch (points).
    pub fn touch(&mut self, id: u64, phase: Phase, x: f32, y: f32) {
        self.app.touch(id, phase, x, y);
    }

    /// Runs the UI and draws it. `time` is seconds on a monotonic clock.
    ///
    /// # Errors
    /// [`ViewError::Gpu`] if drawing failed in a way that needs the shell to react.
    pub fn render(&mut self, time: f64) -> Result<Next, ViewError> {
        #[cfg_attr(not(target_vendor = "apple"), allow(unused_mut))]
        let mut frame = self.app.frame(time);
        #[cfg(target_vendor = "apple")]
        if let Some(gpu) = &mut self.gpu {
            let drawn = gpu
                .paint(&mut frame.full, self.app.ctx(), self.app.viewport())
                .map_err(ViewError::Gpu)?;
            if !drawn {
                // Nothing reached the screen; going idle now would leave it blank.
                return Ok(Next::Immediately);
            }
        }
        Ok(match frame.repaint_after {
            Some(d) if d.is_zero() => Next::Immediately,
            Some(d) => Next::After(d),
            None => Next::Idle,
        })
    }

    /// The app logic.
    #[must_use]
    pub fn app(&self) -> &App {
        &self.app
    }

    /// The app logic, mutable (adapter events, navigation questions, command queue).
    pub fn app_mut(&mut self) -> &mut App {
        &mut self.app
    }
}
