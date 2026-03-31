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
  btn.textContent = 'Opening Settings...';

  // Open Screen Recording settings directly (no system dialog)
  try {
    await invoke('request_permission');
  } catch {}

  // Move to step 2
  step1.className = 'step done';
  step1.querySelector('.step-num')!.textContent = '\u2713';
  step2.className = 'step active';
  btn.textContent = 'Waiting...';
  status.textContent = 'Toggle QuickShotter ON in the Settings window, then come back here';
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

  // After 2 minutes, offer to retry
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
        btn.textContent = 'Waiting...';
        startPolling();
      };
    }
  }, 120000);
}

async function onPermissionGranted() {
  // Mark all steps done
  step1.className = 'step done';
  step1.querySelector('.step-num')!.textContent = '\u2713';
  step2.className = 'step done';
  step2.querySelector('.step-num')!.textContent = '\u2713';
  step3.className = 'step done';
  step3.querySelector('.step-num')!.textContent = '\u2713';

  status.textContent = '';
  status.className = 'status granted';

  // Replace button area with two options
  btn.textContent = 'Start Capturing';
  btn.className = 'btn btn-success';
  btn.disabled = false;
  btn.onclick = async () => {
    try { await invoke('complete_onboarding'); } catch {}
    const win = getCurrentWindow();
    await win.close();
  };

  // Add "Customize Hotkeys" link below the button
  if (!document.getElementById('settings-link')) {
    const link = document.createElement('button');
    link.id = 'settings-link';
    link.textContent = 'Customize Hotkeys';
    link.className = 'btn-link';
    link.onclick = async () => {
      try {
        await invoke('complete_onboarding');
        // Open settings window to shortcuts tab
        const { WebviewWindow } = await import('@tauri-apps/api/webviewWindow');
        const settingsWin = await WebviewWindow.getByLabel('settings');
        if (settingsWin) {
          await settingsWin.show();
          await settingsWin.setFocus();
        }
      } catch {}
      const win = getCurrentWindow();
      await win.close();
    };
    btn.parentElement!.appendChild(link);
  }
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
