import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';

const btn = document.getElementById('grant-btn') as HTMLButtonElement;
const status = document.getElementById('status')!;
const step1 = document.getElementById('step1')!;
const step2 = document.getElementById('step2')!;
const step3 = document.getElementById('step3')!;
const hotkeyHint = document.getElementById('hotkey-hint')!;

let polling = false;

// Load the configured hotkey for display
async function loadHotkey() {
  try {
    const config: { hotkey_region: string } = await invoke('get_config');
    const raw = config.hotkey_region || 'Alt+Backquote';
    hotkeyHint.textContent = raw
      .replace(/CmdOrCtrl/g, '⌘')
      .replace(/Alt/g, '⌥')
      .replace(/Shift/g, '⇧')
      .replace(/Backquote/g, '`')
      .replace(/\+/g, '');
  } catch {}
}

btn.addEventListener('click', async () => {
  btn.disabled = true;

  // Request permission — triggers system dialog or opens settings
  try {
    await invoke('request_permission');
  } catch {}

  // Move to step 2
  step1.className = 'step done';
  step1.querySelector('.step-num')!.textContent = '\u2713';
  step2.className = 'step active';
  status.textContent = 'Waiting for permission...';
  status.className = 'status checking';

  // Start polling for permission
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

  // Stop polling after 2 minutes
  setTimeout(() => {
    if (polling) {
      clearInterval(interval);
      polling = false;
      status.textContent = 'Still waiting? Make sure QuickShotter is toggled ON in Settings.';
      btn.textContent = 'Open Settings Again';
      btn.disabled = false;
      btn.className = 'btn btn-primary';
      btn.onclick = async () => {
        try { await invoke('request_permission'); } catch {}
        btn.disabled = true;
        startPolling();
      };
    }
  }, 120000);
}

async function onPermissionGranted() {
  step2.className = 'step done';
  step2.querySelector('.step-num')!.textContent = '\u2713';
  step3.className = 'step active';

  btn.textContent = 'Start Capturing';
  btn.className = 'btn btn-success';
  btn.disabled = false;
  status.textContent = '\u2713 Screen recording access granted';
  status.className = 'status granted';

  btn.onclick = async () => {
    // Mark onboarding complete so we never show this again
    try { await invoke('complete_onboarding'); } catch {}
    const win = getCurrentWindow();
    await win.close();
  };
}

// Check on load — maybe permission was already granted
window.addEventListener('DOMContentLoaded', async () => {
  loadHotkey();
  try {
    const granted: boolean = await invoke('check_permission');
    if (granted) {
      onPermissionGranted();
    }
  } catch {}
});
