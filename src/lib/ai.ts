export type AiPreset = {
  id: string;
  label: string;
  endpoint: string;
  model: string;
};

/** The same OpenAI-compatible presets DBX offers for hosted models. */
export const AI_PRESETS: AiPreset[] = [
  { id: "deepseek", label: "DeepSeek", endpoint: "https://api.deepseek.com/v1", model: "deepseek-v4-flash" },
  { id: "kimi", label: "Kimi", endpoint: "https://api.moonshot.cn/v1", model: "" },
  { id: "qwen", label: "Qwen", endpoint: "https://dashscope.aliyuncs.com/compatible-mode/v1", model: "qwen-plus" },
  { id: "zhipu", label: "Zhipu", endpoint: "https://open.bigmodel.cn/api/paas/v4", model: "glm-5.3" },
  { id: "minimax", label: "MiniMax", endpoint: "https://api.minimax.io/v1", model: "MiniMax-M3.1-Flash" },
  { id: "openai", label: "OpenAI", endpoint: "https://api.openai.com/v1/chat/completions", model: "gpt-4o-mini" },
  { id: "openai-compatible", label: "OpenAI Compatible", endpoint: "", model: "" },
  { id: "ollama", label: "Ollama", endpoint: "http://localhost:11434/v1", model: "llama3.1" },
];
