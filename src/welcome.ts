import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';

const btn = document.getElementById('grant-btn') as HTMLButtonElement;
const status = document.getElementById('status')!;
const step1 = document.getElementById('step1')!;
const step2 = document.getElementById('step2')!;
const step3 = document.getElementById('step3')!;
const hotkeyDisplay = document.getElementById('hotkey-display')!;

let polling = false;

const keyLabels: Record<string, string> = {
  'CmdOrCtrl': '⌘ Cmd',
  'Alt': '⌥ Option',
  'Shift': '⇧ Shift',
  'Backquote': '`',
};

// Render hotkey as visual keycaps
function renderKeycaps(raw: string) {
  const parts = raw.split('+');
  hotkeyDisplay.innerHTML = parts
    .map(p => {
      const label = keyLabels[p] || p;
      return `<span class="keycap">${label}</span>`;
    })
    .join(' ');
}

async function loadHotkey() {
  try {
    const config: { hotkey_region: string } = await invoke('get_config');
    renderKeycaps(config.hotkey_region || 'Alt+Backquote');
  } catch {}
}

btn.addEventListener('click', async () => {

  // Trigger system permission dialog (adds QuickShotter to the list)
  try {
    await invoke('request_permission');
  } catch {}

  // Move to step 2
  step1.className = 'step done';
  step1.querySelector('.step-num')!.textContent = '\u2713';
  step2.className = 'step active';
  status.textContent = 'Toggle QuickShotter ON in Settings, then come back here';
  status.className = 'status checking';

  startPolling();
});

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
      status.textContent = 'Still waiting? Make sure QuickShotter is toggled ON in Settings.';
      btn.textContent = 'Try Again';
      btn.disabled = false;
      btn.className = 'btn btn-primary';
      btn.onclick = async () => {
        try { await invoke('request_permission'); } catch {}
        btn.disabled = true;
        btn.textContent = 'Waiting...';
        startPolling();
      };
    }
  }, 120000);
}

async function onPermissionGranted() {
  step1.className = 'step done';
  step1.querySelector('.step-num')!.textContent = '\u2713';
  step2.className = 'step done';
  step2.querySelector('.step-num')!.textContent = '\u2713';
  step3.className = 'step done';
  step3.querySelector('.step-num')!.textContent = '\u2713';

  status.textContent = '';
  status.className = 'status granted';

  // Show menu bar hint
  const hint = document.getElementById('menu-hint');
  if (hint) hint.style.display = 'block';

  // Replace Grant Access button with completion buttons
  const btnRow = document.getElementById('btn-row')!;
  btnRow.innerHTML = `
    <button class="btn btn-success" id="start-btn">Start Capturing</button>
    <button class="btn btn-secondary" id="settings-btn">Settings</button>
  `;

  document.getElementById('start-btn')!.addEventListener('click', async () => {
    await invoke('complete_onboarding').catch(() => {});
    await getCurrentWindow().destroy().catch(() => {});
  });

  document.getElementById('settings-btn')!.addEventListener('click', async () => {
    await invoke('complete_onboarding').catch(() => {});
    await invoke('show_settings').catch(() => {});
    await getCurrentWindow().destroy().catch(() => {});
  });
}

window.addEventListener('DOMContentLoaded', async () => {
  loadHotkey();
  try {
    const granted: boolean = await invoke('check_permission');
    if (granted) {
      onPermissionGranted();
    }
  } catch {}
});
