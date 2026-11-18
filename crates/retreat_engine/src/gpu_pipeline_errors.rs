//! Turns GPU pipeline compile errors (a broken shader) into a failed command script.
//!
//! Without this, Bevy only logs the error and stops drawing the meshes that use the broken
//! pipeline, so a check script would still pass with an invisible object. Pipelines compile
//! in the render world, a frame or two after the line that caused them (e.g. `shader.apply`),
//! so the error is reported against the last line that ran.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use bevy::render::render_resource::{CachedPipelineState, PipelineCache};
use bevy::render::{Render, RenderApp, RenderSystems};

use crate::command_script::fail_running_script;

/// Adds the render-world scan and the main-world report. Needs the render sub-app, so
/// `RetreatEnginePlugin` adds it after `DefaultPlugins`.
pub(crate) struct GpuPipelineErrorsPlugin;

impl Plugin for GpuPipelineErrorsPlugin {
    fn build(&self, app: &mut App) {
        let errors = GpuPipelineErrors::default();
        app.insert_resource(errors.clone())
            .add_systems(Last, fail_script_on_gpu_pipeline_errors);
        app.sub_app_mut(RenderApp)
            .insert_resource(errors)
            .init_resource::<ReportedPipelines>()
            .add_systems(
                Render,
                collect_gpu_pipeline_errors.in_set(RenderSystems::Cleanup),
            );
    }
}

/// Error messages passed from the render world to the main world. Both worlds hold a clone
/// of the same `Arc`; the render world may run in parallel with the main world, hence the
/// `Mutex`.
#[derive(Resource, Clone, Default)]
struct GpuPipelineErrors(Arc<Mutex<Vec<String>>>);

/// Indexes (in `PipelineCache::pipelines`) of failed pipelines already reported, so each
/// failure is reported once although it stays in the cache.
#[derive(Resource, Default)]
struct ReportedPipelines(HashSet<usize>);

/// Render world, after rendering: records every pipeline that newly failed to compile.
fn collect_gpu_pipeline_errors(
    pipeline_cache: Res<PipelineCache>,
    errors: Res<GpuPipelineErrors>,
    mut reported: ResMut<ReportedPipelines>,
) {
    for (index, pipeline) in pipeline_cache.pipelines().enumerate() {
        if let CachedPipelineState::Err(error) = &pipeline.state
            && reported.0.insert(index)
        {
            let mut messages = errors.0.lock().expect("no thread panics while holding it");
            messages.push(error_with_causes(error));
        }
    }
}

/// `error` followed by each underlying cause (the top-level text alone is only
/// "Validation Error"; the cause names the bad line of shader code).
fn error_with_causes(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut cause = error.source();
    while let Some(inner) = cause {
        text.push_str(": ");
        text.push_str(&inner.to_string());
        cause = inner.source();
    }
    text
}

/// Main world: fails the running script (if any) with the first new pipeline error. Without a
/// script there is nothing to fail; Bevy has already logged the error.
fn fail_script_on_gpu_pipeline_errors(world: &mut World) {
    let new_errors: Vec<String> = {
        let errors = world.resource::<GpuPipelineErrors>();
        let mut messages = errors.0.lock().expect("no thread panics while holding it");
        messages.drain(..).collect()
    };
    if let Some(first) = new_errors.first() {
        fail_running_script(
            world,
            format!(
                "a GPU pipeline failed to compile (a broken shader? it was caused by this \
                 line or one shortly before): {first}"
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An error with an optional cause, standing in for wgpu's nested shader errors.
    #[derive(Debug)]
    struct Layer {
        text: &'static str,
        cause: Option<Box<Layer>>,
    }

    impl std::fmt::Display for Layer {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str(self.text)
        }
    }

    impl std::error::Error for Layer {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            self.cause
                .as_deref()
                .map(|cause| cause as &(dyn std::error::Error + 'static))
        }
    }

    #[test]
    fn error_with_causes_joins_every_level() {
        let error = Layer {
            text: "Validation Error",
            cause: Some(Box::new(Layer {
                text: "parsing error",
                cause: Some(Box::new(Layer {
                    text: "no definition in scope for identifier: `oops`",
                    cause: None,
                })),
            })),
        };
        assert_eq!(
            error_with_causes(&error),
            "Validation Error: parsing error: no definition in scope for identifier: `oops`"
        );
    }
}
