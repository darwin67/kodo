#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelInfo {
    pub id: &'static str,
    pub display_name: &'static str,
    pub context_k: u32,
}

const OPENAI_MODELS: &[ModelInfo] = &[
    ModelInfo {
        id: "gpt-4o",
        display_name: "GPT-4o",
        context_k: 128,
    },
    ModelInfo {
        id: "gpt-4o-mini",
        display_name: "GPT-4o mini",
        context_k: 128,
    },
    ModelInfo {
        id: "o3",
        display_name: "o3",
        context_k: 200,
    },
    ModelInfo {
        id: "o4-mini",
        display_name: "o4-mini",
        context_k: 200,
    },
];

const ANTHROPIC_MODELS: &[ModelInfo] = &[
    ModelInfo {
        id: "claude-opus-4-5",
        display_name: "Claude Opus 4.5",
        context_k: 200,
    },
    ModelInfo {
        id: "claude-sonnet-4-5",
        display_name: "Claude Sonnet 4.5",
        context_k: 200,
    },
    ModelInfo {
        id: "claude-haiku-4-5",
        display_name: "Claude Haiku 4.5",
        context_k: 200,
    },
];

const GEMINI_MODELS: &[ModelInfo] = &[
    ModelInfo {
        id: "gemini-2.5-pro",
        display_name: "Gemini 2.5 Pro",
        context_k: 1024,
    },
    ModelInfo {
        id: "gemini-2.5-flash",
        display_name: "Gemini 2.5 Flash",
        context_k: 1024,
    },
    ModelInfo {
        id: "gemini-2.0-flash",
        display_name: "Gemini 2.0 Flash",
        context_k: 1024,
    },
];

const EMPTY_MODELS: &[ModelInfo] = &[];

pub fn available_models(provider_kind: &str) -> &'static [ModelInfo] {
    match provider_kind {
        "openai" => OPENAI_MODELS,
        "anthropic" => ANTHROPIC_MODELS,
        "gemini" => GEMINI_MODELS,
        _ => EMPTY_MODELS,
    }
}

#[cfg(test)]
mod tests {
    use super::available_models;

    #[test]
    fn openai_has_expected_models() {
        assert!(available_models("openai").len() >= 4);
    }

    #[test]
    fn anthropic_has_expected_models() {
        assert!(available_models("anthropic").len() >= 3);
    }

    #[test]
    fn gemini_has_expected_models() {
        assert!(available_models("gemini").len() >= 3);
    }

    #[test]
    fn unknown_provider_returns_empty_slice() {
        assert!(available_models("unknown").is_empty());
    }
}
