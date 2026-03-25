import './countdown.css';
import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';

const el = document.getElementById('timer')!;
let cancelled = false;

// Focus the window so we receive keyboard events
getCurrentWindow().setFocus().catch(() => {});

document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape') {
    cancelled = true;
    invoke('cancel_delayed_capture').catch(() => {});
  }
});

async function run() {
  let delay: number;
  try {
    delay = await invoke<number>('get_capture_delay');
  } catch {
    delay = 3;
  }

  let remaining = delay;
  const step = 0.1;
  el.textContent = remaining.toFixed(1);

  await new Promise<void>((resolve) => {
    const interval = setInterval(() => {
      if (cancelled) { clearInterval(interval); return; }
      remaining = Math.max(0, remaining - step);
      el.textContent = remaining.toFixed(1);
      if (remaining <= 0) {
        clearInterval(interval);
        resolve();
      }
    }, step * 1000);
  });

  if (cancelled) return;

  // Timer done — tell backend to execute the capture
  invoke('execute_delayed_capture').catch((e) => {
    console.error('Delayed capture failed:', e);
  });
}

run();
