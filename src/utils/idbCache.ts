type StoreName = 'cf_mods';

interface CacheRecord<T> {
  key: string;
  ts: number;
  value: T;
}

const DB_NAME = 'bmcbl_cache';
const DB_VERSION = 1;

let dbPromise: Promise<IDBDatabase> | null = null;

function openDb(): Promise<IDBDatabase> {
  if (dbPromise) return dbPromise;
  dbPromise = new Promise((resolve, reject) => {
    const req = indexedDB.open(DB_NAME, DB_VERSION);
    req.onupgradeneeded = () => {
      const db = req.result;
      if (!db.objectStoreNames.contains('cf_mods')) {
        db.createObjectStore('cf_mods', { keyPath: 'key' });
      }
    };
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
  return dbPromise;
}

async function withStore<T>(
  store: StoreName,
  mode: IDBTransactionMode,
  fn: (os: IDBObjectStore) => IDBRequest<T> | void
): Promise<T | void> {
  const db = await openDb();
  return new Promise((resolve, reject) => {
    const tx = db.transaction(store, mode);
    const os = tx.objectStore(store);
    const req = fn(os) as IDBRequest<T> | undefined;
    if (req) {
      req.onsuccess = () => resolve(req.result);
      req.onerror = () => reject(req.error);
    }
    tx.oncomplete = () => resolve(undefined);
    tx.onerror = () => reject(tx.error);
    tx.onabort = () => reject(tx.error);
  });
}

export async function idbCacheGet<T>(store: StoreName, key: string): Promise<CacheRecord<T> | null> {
  try {
    const res = (await withStore<CacheRecord<T> | undefined>(store, 'readonly', (os) => os.get(key))) as
      | CacheRecord<T>
      | undefined;
    return res ?? null;
  } catch {
    return null;
  }
}

export async function idbCacheSet<T>(store: StoreName, key: string, value: T): Promise<void> {
  try {
    await withStore(store, 'readwrite', (os) => os.put({ key, ts: Date.now(), value }));
  } catch {
    // ignore (quota / disabled storage)
  }
}

export async function idbCacheDelete(store: StoreName, key: string): Promise<void> {
  try {
    await withStore(store, 'readwrite', (os) => os.delete(key));
  } catch {
    // ignore
  }
}

export async function idbCachePrune(store: StoreName, maxAgeMs: number): Promise<void> {
  try {
    const db = await openDb();
    await new Promise<void>((resolve, reject) => {
      const tx = db.transaction(store, 'readwrite');
      const os = tx.objectStore(store);
      const req = os.openCursor();
      req.onsuccess = () => {
        const cursor = req.result;
        if (!cursor) return;
        const rec = cursor.value as CacheRecord<unknown>;
        if (!rec?.ts || Date.now() - rec.ts > maxAgeMs) cursor.delete();
        cursor.continue();
      };
      req.onerror = () => reject(req.error);
      tx.oncomplete = () => resolve();
      tx.onerror = () => reject(tx.error);
      tx.onabort = () => reject(tx.error);
    });
  } catch {
    // ignore
  }
}
