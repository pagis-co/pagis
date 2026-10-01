/** The engine behind each Docker discovery source (ADR-0024). */
const SOURCE_NAMES: Record<string, string> = {
  override: 'Your endpoint',
  environment: 'DOCKER_HOST',
  context: 'Docker context',
  docker_desktop: 'Docker Desktop',
  orbstack: 'OrbStack',
  colima: 'Colima',
  rancher_desktop: 'Rancher Desktop',
  lima: 'Lima',
  rootless_docker: 'Rootless Docker',
  system_socket: 'System socket',
  podman: 'Podman',
}

/** The name the user knows a discovery source by. */
export function dockerSourceName(source: string): string {
  return SOURCE_NAMES[source] ?? source
}
