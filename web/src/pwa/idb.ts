// One small IndexedDB shared by the page and the service worker. public/sw.js opens the same
// database (share-target inbox) and creates the same stores; keep DB_NAME, DB_VERSION and the
// store layout in step with it.

export const DB_NAME = "openside-pwa";
const DB_VERSION = 1;
export const OUTBOX_STORE = "outbox";
export const SHARE_STORE = "share";

type StoreName = typeof OUTBOX_STORE | typeof SHARE_STORE;

const KEY_PATHS: Record<StoreName, string> = { [OUTBOX_STORE]: "message_id", [SHARE_STORE]: "id" };

let opened: Promise<IDBDatabase> | null = null;

export function openDatabase(): Promise<IDBDatabase> {
  if (opened) return opened;
  const { promise, resolve, reject } = Promise.withResolvers<IDBDatabase>();
  opened = promise;
  const request = indexedDB.open(DB_NAME, DB_VERSION);
  request.onupgradeneeded = () => {
    for (const name of [OUTBOX_STORE, SHARE_STORE] as const) {
      if (!request.result.objectStoreNames.contains(name)) request.result.createObjectStore(name, { keyPath: KEY_PATHS[name] });
    }
  };
  request.onsuccess = () => {
    // A newer deployment upgrading the schema must not be blocked by this tab.
    request.result.onversionchange = () => {
      request.result.close();
      opened = null;
    };
    resolve(request.result);
  };
  request.onerror = () => {
    opened = null;
    reject(request.error ?? new Error("IndexedDB is unavailable"));
  };
  return promise;
}

function wrap<T>(request: IDBRequest<T>): Promise<T> {
  const { promise, resolve, reject } = Promise.withResolvers<T>();
  request.onsuccess = () => resolve(request.result);
  request.onerror = () => reject(request.error);
  return promise;
}

export async function idbAll<T>(store: StoreName): Promise<T[]> {
  const db = await openDatabase();
  return wrap<T[]>(db.transaction(store).objectStore(store).getAll());
}

export async function idbGet<T>(store: StoreName, key: string): Promise<T | undefined> {
  const db = await openDatabase();
  return wrap<T | undefined>(db.transaction(store).objectStore(store).get(key));
}

export async function idbPut(store: StoreName, value: unknown): Promise<void> {
  const db = await openDatabase();
  await wrap(db.transaction(store, "readwrite").objectStore(store).put(value));
}

export async function idbDelete(store: StoreName, key: string): Promise<void> {
  const db = await openDatabase();
  await wrap(db.transaction(store, "readwrite").objectStore(store).delete(key));
}

/** What the outbox needs from persistence; tests swap in a Map, the app uses IndexedDB. */
export interface RecordStore<T> {
  all(): Promise<T[]>;
  put(value: T): Promise<void>;
  delete(key: string): Promise<void>;
}

export function idbStore<T>(store: StoreName): RecordStore<T> {
  return { all: () => idbAll<T>(store), put: (value) => idbPut(store, value), delete: (key) => idbDelete(store, key) };
}
