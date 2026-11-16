/** Every seed write, as `[db, ...redis-cli args]`. */
export function seedCommands(): string[][]
/** Runs every seed write through `redis-cli`, optionally inside a container. */
export function seed(options: { port: number | string; container?: string }): void
