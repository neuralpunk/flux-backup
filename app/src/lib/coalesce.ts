// Run a reload once for a burst of reasons. A finished backup, for example, both bumps
// `app.revision` and refreshes the backup list a moment later; each used to reload the
// view (and open an encrypted repository) on its own.

/** `run()` asks for `fn`: calls within `ms` of each other become one, and a call while `fn`
 * is running schedules exactly one more run after it. The first call runs at once. */
export function coalesce(fn: () => Promise<unknown>, ms = 150) {
  let timer: ReturnType<typeof setTimeout> | undefined;
  let running = false;
  let again = false;
  let first = true;

  async function go() {
    timer = undefined;
    if (running) {
      again = true;
      return;
    }
    running = true;
    try {
      await fn();
    } finally {
      running = false;
      if (again) {
        again = false;
        void go();
      }
    }
  }

  return {
    run() {
      if (first) {
        first = false;
        void go();
        return;
      }
      clearTimeout(timer);
      timer = setTimeout(() => void go(), ms);
    },
    cancel() {
      clearTimeout(timer);
      timer = undefined;
    },
  };
}
