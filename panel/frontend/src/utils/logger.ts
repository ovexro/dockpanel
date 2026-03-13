// Logger utility that suppresses error/warn output in production builds.
// Keeps console.log available for intentional messages.
export const logger = {
  error: (...args: unknown[]) => {
    if (import.meta.env.DEV) console.error(...args);
  },
  warn: (...args: unknown[]) => {
    if (import.meta.env.DEV) console.warn(...args);
  },
};
