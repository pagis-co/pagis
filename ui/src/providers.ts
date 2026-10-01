// The display names of the model providers that the daemon holds keys
// for. The daemon names a provider by its id: `anthropic`, `openai`,
// `openrouter`.

const PROVIDER_NAMES: Record<string, string> = {
  anthropic: 'Anthropic',
  openai: 'OpenAI',
  openrouter: 'OpenRouter',
}

/** The name a person reads for a provider id. An unknown id reads as
 *  itself. */
export function providerName(provider: string): string {
  return PROVIDER_NAMES[provider] ?? provider
}

/** "Anthropic, OpenAI or OpenRouter": the providers of `ids`, once
 *  each, as a choice. */
export function providersOr(ids: string[]): string {
  const names = [...new Set(ids)].map(providerName)
  return names.length < 2
    ? names.join('')
    : `${names.slice(0, -1).join(', ')} or ${names[names.length - 1]}`
}
