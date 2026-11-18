export function shortcutKeys(value: string): string[] {
  return value
    .split("+")
    .map((part) => part.trim())
    .filter(Boolean)
    .map(prettyToken);
}

export function shortcutLabel(value: string): string {
  return shortcutKeys(value).join("+");
}

function prettyToken(token: string): string {
  if (token === "Control") return "Ctrl";
  if (token === "Super") return "Win";
  if (token.startsWith("Key") && token.length === 4) return token.slice(3);
  if (token.startsWith("Digit") && token.length === 6) return token.slice(5);
  return token;
}
