import { invoke } from '@tauri-apps/api/core';

let polling = false;

const keyLabels: Record<string, string> = {
  'CmdOrCtrl': '⌘ Cmd',
  'Alt': '⌥ Option',
  'Shift': '⇧ Shift',
  'Backquote': '`',
};

function renderKeycaps(raw: string) {
  const el = document.getElementById('hotkey-display')!;
  el.innerHTML = raw.split('+')
    .map(p => `<span class="keycap">${keyLabels[p] || p}</span>`)
    .join(' ');
}

async function loadHotkey() {
  try {
    const config: { hotkey_region: string } = await invoke('get_config');
    renderKeycaps(config.hotkey_region || 'Alt+Backquote');
  } catch {}
}

function onPermissionGranted() {
  document.getElementById('step1')!.className = 'step done';
  document.getElementById('step1')!.querySelector('.step-num')!.textContent = '\u2713';
  document.getElementById('step2')!.className = 'step done';
  document.getElementById('step2')!.querySelector('.step-num')!.textContent = '\u2713';
  document.getElementById('step3')!.className = 'step done';
  document.getElementById('step3')!.querySelector('.step-num')!.textContent = '\u2713';

  document.getElementById('status')!.textContent = '';
  const hint = document.getElementById('menu-hint');
  if (hint) hint.style.display = 'block';

  // Swap buttons
  document.getElementById('grant-btn')!.style.display = 'none';
  document.getElementById('start-btn')!.style.display = '';
  document.getElementById('settings-btn')!.style.display = '';
}

function startPolling() {
  if (polling) return;
  polling = true;

  const interval = setInterval(async () => {
    try {
      const granted: boolean = await invoke('check_permission');
      if (granted) {
        clearInterval(interval);
        polling = false;
        onPermissionGranted();
      }
    } catch {}
  }, 1000);

  setTimeout(() => {
    if (polling) {
      clearInterval(interval);
      polling = false;
      document.getElementById('status')!.textContent =
        'Still waiting? Make sure QuickShotter is toggled ON in Settings.';
    }
  }, 120000);
}

window.addEventListener('DOMContentLoaded', async () => {
  loadHotkey();

  // Grant Access button
  document.getElementById('grant-btn')!.addEventListener('click', async () => {
    try { await invoke('request_permission'); } catch {}

    document.getElementById('step1')!.className = 'step done';
    document.getElementById('step1')!.querySelector('.step-num')!.textContent = '\u2713';
    document.getElementById('step2')!.className = 'step active';
    document.getElementById('status')!.textContent =
      'Toggle QuickShotter ON in Settings, then come back here';
    document.getElementById('status')!.className = 'status checking';

    startPolling();
  });

  // Start Capturing — Rust closes the window
  document.getElementById('start-btn')!.addEventListener('click', () => {
    invoke('complete_onboarding');
  });

  // Settings — Rust closes the window and opens settings
  document.getElementById('settings-btn')!.addEventListener('click', () => {
    invoke('show_settings');
    invoke('complete_onboarding');
  });

  // Check if already granted on load
  try {
    const granted: boolean = await invoke('check_permission');
    if (granted) {
      onPermissionGranted();
    }
  } catch {}
});
