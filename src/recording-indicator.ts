import './recording-indicator.css';
import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';

interface RecordingState {
  is_recording: boolean;
  elapsed_secs: number;
  format: string;
  saved_filepath: string | null;
}

function formatTime(secs: number): string {
  const m = Math.floor(secs / 60);
  const s = Math.floor(secs % 60);
  return `${m}:${s.toString().padStart(2, '0')}`;
}

// Module scripts execute after DOM parsing -- no need for DOMContentLoaded
async function init() {
  const timerEl = document.getElementById('timer')!;
  const stopBtn = document.getElementById('stop-btn')!;
  const recordingState = document.getElementById('recording-state')!;
  const savedState = document.getElementById('saved-state')!;
  const savedLabel = document.getElementById('saved-label')!;
  const folderBtn = document.getElementById('folder-btn')!;
  const closeBtn = document.getElementById('close-btn')!;
  const cancelBtn = document.getElementById('cancel-btn')!;

  let stoppingManually = false;
  let savedFilepath: string | null = null;

  function closeWindow() {
    // Try both JS destroy and Rust-side close as belt-and-suspenders
    getCurrentWindow().destroy().catch(() => {});
  }

  function showSavedState(filepath: string | null) {
    savedFilepath = filepath;
    recordingState.style.display = 'none';
    savedState.classList.add('visible');
    document.querySelector('.indicator')?.classList.add('saved');

    if (filepath) {
      const filename = filepath.split(/[\\/]/).pop() || 'recording';
      savedLabel.textContent = filename;
    } else {
      savedLabel.textContent = 'Saved';
    }

    // Countdown auto-close
    let countdown = 3;
    const countdownEl = document.getElementById('countdown')!;
    countdownEl.textContent = `${countdown}s`;
    const tick = setInterval(() => {
      countdown--;
      if (countdown <= 0) {
        clearInterval(tick);
        closeWindow();
      } else {
        countdownEl.textContent = `${countdown}s`;
      }
    }, 1000);
  }

  closeBtn.addEventListener('click', () => {
    console.log('[indicator] close-btn clicked');
    closeWindow();
  });

  cancelBtn.addEventListener('click', async () => {
    console.log('[indicator] cancel-btn (x) clicked');
    stoppingManually = true;
    cancelBtn.textContent = '...';
    try {
      await invoke('stop_recording');
      console.log('[indicator] stop_recording returned');
    } catch (e) { console.log('[indicator] stop_recording error:', e); }
    closeWindow();
  });

  folderBtn.addEventListener('click', async () => {
    if (savedFilepath) {
      await invoke('reveal_file', { path: savedFilepath }).catch(() => {});
    }
    closeWindow();
  });

  stopBtn.addEventListener('click', async () => {
    console.log('[indicator] stop-btn clicked');
    stoppingManually = true;
    stopBtn.textContent = '...';
    try {
      const result = await invoke<{ filepath: string | null }>('stop_recording');
      console.log('[indicator] stop_recording result:', result);
      showSavedState(result?.filepath ?? null);
    } catch (e) {
      console.error('[indicator] Stop recording failed:', e);
      closeWindow();
    }
  });

  // Debug: track focus state
  window.addEventListener('focus', () => console.log('[indicator] window focused'));
  window.addEventListener('blur', () => console.log('[indicator] window blurred'));
  document.addEventListener('mousedown', (e) => {
    const target = e.target as HTMLElement;
    console.log('[indicator] mousedown on:', target.tagName, target.className, target.id);
  });

  // Escape key stops recording
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape' && !stoppingManually) {
      stopBtn.click();
    }
  });

  // Poll recording state to update timer
  const interval = setInterval(async () => {
    if (stoppingManually) {
      clearInterval(interval);
      return;
    }
    try {
      const state: RecordingState = await invoke('get_recording_state');
      if (!state.is_recording) {
        clearInterval(interval);
        // Recording was stopped externally (hotkey/tray).
        // Poll briefly for the saved filepath to become available.
        pollForFilepath();
        return;
      }
      timerEl.textContent = formatTime(state.elapsed_secs);
    } catch (e) {
      console.error('get_recording_state failed:', e);
      clearInterval(interval);
      closeWindow();
    }
  }, 500);

  // After external stop, poll until the filepath appears in state
  function pollForFilepath() {
    stopBtn.textContent = '...';
    let attempts = 0;
    const maxAttempts = 60; // 30 seconds max (GIF encoding can take time)

    const fpInterval = setInterval(async () => {
      attempts++;
      try {
        const state: RecordingState = await invoke('get_recording_state');
        if (state.saved_filepath || attempts >= maxAttempts) {
          clearInterval(fpInterval);
          showSavedState(state.saved_filepath);
        }
      } catch {
        clearInterval(fpInterval);
        closeWindow();
      }
    }, 500);
  }
}

init().catch((e) => {
  console.error('Recording indicator init failed:', e);
  // Show error in the timer element so it's visible to the user
  const timerEl = document.getElementById('timer');
  if (timerEl) timerEl.textContent = 'ERR';
});
