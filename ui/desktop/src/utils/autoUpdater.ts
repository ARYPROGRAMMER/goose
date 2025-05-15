import { ipcRenderer } from 'electron';

interface UpdateInfo {
  version: string;
  releaseNotes?: string;
}

export interface UpdateStatus {
  checking: boolean;
  available: boolean;
  downloading: boolean;
  downloaded: boolean;
  error?: string;
}

// Event names for update events
export const UPDATE_EVENTS = {
  CHECK_FOR_UPDATE: 'check-for-update',
  UPDATE_AVAILABLE: 'update-available',
  UPDATE_NOT_AVAILABLE: 'update-not-available',
  UPDATE_DOWNLOADED: 'update-downloaded',
  UPDATE_ERROR: 'update-error',
  DOWNLOAD_PROGRESS: 'download-progress',
};

class AutoUpdater {
  private status: UpdateStatus = {
    checking: false,
    available: false,
    downloading: false,
    downloaded: false,
  };

  private listeners: Map<string, Set<Function>> = new Map();

  constructor() {
    this.setupEventListeners();
  }

  private setupEventListeners() {
    // Listen for update events from the main process
    Object.values(UPDATE_EVENTS).forEach(eventName => {
      ipcRenderer.on(eventName, (_, ...args) => {
        this.emit(eventName, ...args);
      });
    });
  }

  public checkForUpdates() {
    this.status.checking = true;
    this.emit('status-change', this.status);
    ipcRenderer.send(UPDATE_EVENTS.CHECK_FOR_UPDATE);
  }

  public getStatus(): UpdateStatus {
    return { ...this.status };
  }

  public on(event: string, callback: Function) {
    if (!this.listeners.has(event)) {
      this.listeners.set(event, new Set());
    }
    this.listeners.get(event)?.add(callback);
  }

  public off(event: string, callback: Function) {
    this.listeners.get(event)?.delete(callback);
  }

  private emit(event: string, ...args: any[]) {
    this.listeners.get(event)?.forEach(callback => callback(...args));
  }

  public destroy() {
    // Clean up event listeners
    Object.values(UPDATE_EVENTS).forEach(eventName => {
      ipcRenderer.removeAllListeners(eventName);
    });
    this.listeners.clear();
  }
}

export const autoUpdater = new AutoUpdater();