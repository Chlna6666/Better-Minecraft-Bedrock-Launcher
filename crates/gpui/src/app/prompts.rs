use crate::{
    App, PromptBuilder, PromptButton, PromptHandle, PromptLevel, RenderablePromptHandle, Window,
};

impl App {
    /// Replace platform prompts with a custom prompt renderer.
    pub fn set_prompt_builder(
        &mut self,
        renderer: impl Fn(
            PromptLevel,
            &str,
            Option<&str>,
            &[PromptButton],
            PromptHandle,
            &mut Window,
            &mut App,
        ) -> RenderablePromptHandle
        + 'static,
    ) {
        self.prompt_builder = Some(PromptBuilder::Custom(Box::new(renderer)));
    }

    /// Restore the default prompt renderer.
    pub fn reset_prompt_builder(&mut self) {
        self.prompt_builder = Some(PromptBuilder::Default);
    }
}
