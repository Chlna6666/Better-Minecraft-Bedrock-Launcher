import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { Settings as SettingsIcon, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { useToast } from "../../components/Toast";
import "./OnlinePage.css";

const LEGACY_DEFAULT_BOOTSTRAP_PEER = "tcp://public.easytier.top:11010";
const DEFAULT_BOOTSTRAP_PEER = "tcp://39.108.52.138:11010\ntcp://8.148.29.206:11010";
const PLAYER_NAME_STORAGE_KEY = "bmcbL.online.playerName";
const LEGACY_PLAYER_NAME_STORAGE_KEY = "bmcbk.online.playerName";
const BOOTSTRAP_PEER_STORAGE_KEY = "bmcbL.online.bootstrapPeer";
const DISABLE_P2P_STORAGE_KEY = "bmcbL.online.disableP2p";
const NO_TUN_STORAGE_KEY = "bmcbL.online.noTun";
const GAME_PORT_STORAGE_KEY = "bmcbL.online.gamePort";
const GAME_PORTS_STORAGE_KEY = "bmcbL.online.gamePorts";
const JOIN_ROOM_CODE_STORAGE_KEY = "bmcbL.online.joinRoomCode";
const HOST_ROOM_STORAGE_KEY = "bmcbL.online.hostRoom";
const ACTIVE_ROOM_STORAGE_KEY = "bmcbL.online.activeRoom";
const BASE34_ALPHABET = "0123456789ABCDEFGHJKLMNPQRSTUVWXYZ";
const DEFAULT_GAME_PORTS = "7551";
const DEFAULT_JOIN_UDP_PORT_FALLBACK = 19132;

function normalizePlayerName(name: string): string {
  return String(name || "")
    .trim()
    .replace(/\s+/g, " ")
    .slice(0, 32);
}

function parsePortList(input: string): number[] {
  const text = String(input || "");
  const parts = text.split(/[\s,]+/g).map((s) => s.trim()).filter(Boolean);
  const out: number[] = [];
  const seen = new Set<number>();
  for (const p of parts) {
    const n = Number.parseInt(p, 10);
    if (!Number.isFinite(n) || n <= 0 || n > 65535) continue;
    if (seen.has(n)) continue;
    seen.add(n);
    out.push(n);
  }
  return out;
}

function normalizePortListText(input: string, fallbackText: string): string {
  const ports = parsePortList(input);
  if (ports.length === 0) return fallbackText;
  return ports.join(", ");
}

function migrateLegacyAutoName(name: string): string {
  const trimmed = String(name || "").trim();
  const m = /^BMCBL\s+user\s+([0-9A-Z]{4})$/i.exec(trimmed);
  if (m) return `BMCBL_USER_${m[1].toUpperCase()}`;
  return trimmed;
}

function randomBase34(length: number): string {
  const alphabet = BASE34_ALPHABET;
  try {
    const bytes = new Uint8Array(length);
    crypto.getRandomValues(bytes);
    return Array.from(bytes, (b) => alphabet[b % alphabet.length]).join("");
  } catch {
    let out = "";
    for (let i = 0; i < length; i++) {
      out += alphabet[Math.floor(Math.random() * alphabet.length)];
    }
    return out;
  }
}

function generateDefaultPlayerName(): string {
  return `BMCBL_USER_${randomBase34(4)}`;
}

type PaperConnectRoom = {
  roomCode: string;
  networkName: string;
  networkSecret: string;
};

type PaperConnectCenter = {
  ipv4?: string | null;
  hostname: string;
  port: number;
};

type PlayerEntry = {
  player: string;
  clientId: string;
  isRoomHost: boolean;
  firstSeenMs?: number;
  lastSeenMs?: number;
  returnTime?: number;
};

type EasyTierPeer = { ipv4?: string | null; hostname: string };

type EasyTierEmbeddedStatus = {
  instanceId: string;
  hostname: string;
  ipv4?: string | null;
};

export default function OnlinePage() {
  const { t } = useTranslation();
  const toast = useToast();

  const [bootstrapPeer, setBootstrapPeer] = useState<string>(() => {
    try {
      const stored = String(localStorage.getItem(BOOTSTRAP_PEER_STORAGE_KEY) || "").trim();
      if (!stored) return DEFAULT_BOOTSTRAP_PEER;
      if (stored === LEGACY_DEFAULT_BOOTSTRAP_PEER) {
        localStorage.setItem(BOOTSTRAP_PEER_STORAGE_KEY, DEFAULT_BOOTSTRAP_PEER);
        return DEFAULT_BOOTSTRAP_PEER;
      }
      return stored;
    } catch {
      return DEFAULT_BOOTSTRAP_PEER;
    }
  });
  const [disableP2P, setDisableP2P] = useState<boolean>(() => {
    try {
      const stored = localStorage.getItem(DISABLE_P2P_STORAGE_KEY);
      if (stored === null) return true;
      return stored === "1" || stored === "true";
    } catch {
      return true;
    }
  });
  const [noTun, setNoTun] = useState<boolean>(() => {
    try {
      const stored = localStorage.getItem(NO_TUN_STORAGE_KEY);
      if (stored === null) return true;
      return stored === "1" || stored === "true";
    } catch {
      return true;
    }
  });

  const [easyTierSettingsOpen, setEasyTierSettingsOpen] = useState<boolean>(false);
  const [bootstrapPeerDraft, setBootstrapPeerDraft] = useState<string>(DEFAULT_BOOTSTRAP_PEER);
  const [disableP2PDraft, setDisableP2PDraft] = useState<boolean>(true);
  const [noTunDraft, setNoTunDraft] = useState<boolean>(true);

  const [joinRoomCode, setJoinRoomCode] = useState<string>(() => {
    try {
      return String(localStorage.getItem(JOIN_ROOM_CODE_STORAGE_KEY) || "");
    } catch {
      return "";
    }
  });
  const [hostRoom, setHostRoom] = useState<PaperConnectRoom | null>(() => {
    try {
      const raw = localStorage.getItem(HOST_ROOM_STORAGE_KEY);
      if (!raw) return null;
      const parsed = JSON.parse(raw);
      const roomCode = String(parsed?.roomCode || "").trim();
      const networkName = String(parsed?.networkName || "").trim();
      const networkSecret = String(parsed?.networkSecret || "").trim();
      if (!roomCode || !networkName || !networkSecret) return null;
      return { roomCode, networkName, networkSecret };
    } catch {
      return null;
    }
  });
  const [activeRoom, setActiveRoom] = useState<PaperConnectRoom | null>(() => {
    try {
      const raw = localStorage.getItem(ACTIVE_ROOM_STORAGE_KEY);
      if (!raw) return null;
      const parsed = JSON.parse(raw);
      const roomCode = String(parsed?.roomCode || "").trim();
      const networkName = String(parsed?.networkName || "").trim();
      const networkSecret = String(parsed?.networkSecret || "").trim();
      if (!roomCode || !networkName || !networkSecret) return null;
      return { roomCode, networkName, networkSecret };
    } catch {
      return null;
    }
  });

  const [pcPort, setPcPort] = useState<number>(0);
  const [gamePortsText, setGamePortsText] = useState<string>(() => {
    try {
      const storedList = String(localStorage.getItem(GAME_PORTS_STORAGE_KEY) || "").trim();
      if (storedList) return storedList;
      const storedLegacy = Number(localStorage.getItem(GAME_PORT_STORAGE_KEY) || 0);
      if (storedLegacy > 0) return String(storedLegacy);
      return DEFAULT_GAME_PORTS;
    } catch {
      return DEFAULT_GAME_PORTS;
    }
  });
  const gamePorts = useMemo(() => {
    const ports = parsePortList(gamePortsText);
    return ports.length > 0 ? ports : parsePortList(DEFAULT_GAME_PORTS);
  }, [gamePortsText]);
  const primaryGamePort = useMemo(() => gamePorts[0] ?? 7551, [gamePorts]);

  const [playerName, setPlayerName] = useState<string>(() => {
    try {
      const stored =
        String(localStorage.getItem(PLAYER_NAME_STORAGE_KEY) || "") ||
        String(localStorage.getItem(LEGACY_PLAYER_NAME_STORAGE_KEY) || "");
      const normalized = normalizePlayerName(migrateLegacyAutoName(stored));
      if (normalized) {
        localStorage.setItem(PLAYER_NAME_STORAGE_KEY, normalized);
        return normalized;
      }
    } catch {
      // ignore
    }
    const generated = generateDefaultPlayerName();
    try {
      localStorage.setItem(PLAYER_NAME_STORAGE_KEY, generated);
    } catch {
      // ignore
    }
    return generated;
  });
  const [clientId, setClientId] = useState<string>("");

  const [center, setCenter] = useState<PaperConnectCenter | null>(null);
  const [players, setPlayers] = useState<PlayerEntry[]>([]);
  const [peers, setPeers] = useState<EasyTierPeer[]>([]);
  const [gameEndpoint, setGameEndpoint] = useState<{ ip: string; port: number } | null>(null);
  const [latencyMs, setLatencyMs] = useState<number | null>(null);
  const [running, setRunning] = useState<boolean>(false);
  const [runningRole, setRunningRole] = useState<"host" | "join" | null>(null);
  const [statusText, setStatusText] = useState<string>("");

  const heartbeatTimerRef = useRef<number | null>(null);
  const peersTimerRef = useRef<number | null>(null);
  const hostIdentityRef = useRef<{ playerName: string; clientId: string } | null>(null);

  const hostnameForHost = useMemo(() => (pcPort > 0 ? `paper-connect-server-${pcPort}` : ""), [pcPort]);

  useEffect(() => {
    invoke("paperconnect_default_client_id")
      .then((v) => {
        const next = String(v || "").trim();
        if (!next) return;
        setClientId(next);
      })
      .catch(async () => {
        // Fallback to backend-reported app version (from Cargo.toml) if possible.
        try {
          const ver = String(await invoke("get_app_version")).trim();
          if (ver) setClientId(`BMCBL v${ver}`);
          else setClientId("BMCBL");
        } catch {
          setClientId("BMCBL");
        }
      });
  }, []);

  useEffect(() => {
    try {
      const next = normalizePlayerName(playerName);
      if (!next) return;
      localStorage.setItem(PLAYER_NAME_STORAGE_KEY, next);
    } catch {
      // ignore
    }
  }, [playerName]);

  useEffect(() => {
    try {
      localStorage.setItem(BOOTSTRAP_PEER_STORAGE_KEY, String(bootstrapPeer || "").trim() || DEFAULT_BOOTSTRAP_PEER);
    } catch {
      // ignore
    }
  }, [bootstrapPeer]);

  useEffect(() => {
    try {
      localStorage.setItem(DISABLE_P2P_STORAGE_KEY, disableP2P ? "1" : "0");
    } catch {
      // ignore
    }
  }, [disableP2P]);

  useEffect(() => {
    try {
      localStorage.setItem(NO_TUN_STORAGE_KEY, noTun ? "1" : "0");
    } catch {
      // ignore
    }
  }, [noTun]);

  useEffect(() => {
    try {
      const normalized = normalizePortListText(gamePortsText, DEFAULT_GAME_PORTS);
      localStorage.setItem(GAME_PORTS_STORAGE_KEY, normalized);
      localStorage.setItem(GAME_PORT_STORAGE_KEY, String(primaryGamePort));
    } catch {
      // ignore
    }
  }, [gamePortsText, primaryGamePort]);

  useEffect(() => {
    try {
      localStorage.setItem(JOIN_ROOM_CODE_STORAGE_KEY, String(joinRoomCode || ""));
    } catch {
      // ignore
    }
  }, [joinRoomCode]);

  useEffect(() => {
    try {
      if (hostRoom) localStorage.setItem(HOST_ROOM_STORAGE_KEY, JSON.stringify(hostRoom));
      else localStorage.removeItem(HOST_ROOM_STORAGE_KEY);
    } catch {
      // ignore
    }
  }, [hostRoom]);

  useEffect(() => {
    try {
      if (activeRoom) localStorage.setItem(ACTIVE_ROOM_STORAGE_KEY, JSON.stringify(activeRoom));
      else localStorage.removeItem(ACTIVE_ROOM_STORAGE_KEY);
    } catch {
      // ignore
    }
  }, [activeRoom]);

  const parsePeers = useCallback((text: string): string[] => {
    return String(text || "")
      .split(/[\s,;]+/g)
      .map((s) => s.trim())
      .filter(Boolean);
  }, []);

  const openEasyTierSettings = useCallback(() => {
    setBootstrapPeerDraft(bootstrapPeer);
    setDisableP2PDraft(disableP2P);
    setNoTunDraft(noTun);
    setEasyTierSettingsOpen(true);
  }, [bootstrapPeer, disableP2P, noTun]);

  const closeEasyTierSettings = useCallback(() => {
    setEasyTierSettingsOpen(false);
  }, []);

  const applyEasyTierSettings = useCallback(() => {
    setBootstrapPeer(String(bootstrapPeerDraft || "").trim());
    setDisableP2P(Boolean(disableP2PDraft));
    setNoTun(Boolean(noTunDraft));
    setEasyTierSettingsOpen(false);
  }, [bootstrapPeerDraft, disableP2PDraft, noTunDraft]);

  useEffect(() => {
    if (!easyTierSettingsOpen) return;
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") closeEasyTierSettings();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [closeEasyTierSettings, easyTierSettingsOpen]);

  const appendStatus = useCallback((line: string) => {
    setStatusText((prev) => (prev ? `${prev}\n${line}` : line));
  }, []);

  const formatDuration = useCallback((ms: number): string => {
    const s = Math.max(0, Math.floor(ms / 1000));
    const h = Math.floor(s / 3600);
    const m = Math.floor((s % 3600) / 60);
    const ss = s % 60;
    if (h > 0) return `${h}h ${m}m`;
    if (m > 0) return `${m}m ${ss}s`;
    return `${ss}s`;
  }, []);

  const calcHeartbeatText = useCallback(
    (returnTime: number | null, p: any): { last: string; online: string } | null => {
      const now = Number(returnTime || Date.now());
      const lastSeenMs = Number(p?.lastSeenMs ?? p?.last_seen_ms ?? 0);
      const firstSeenMs = Number(p?.firstSeenMs ?? p?.first_seen_ms ?? 0);
      if (!lastSeenMs || !firstSeenMs) return null;
      return {
        last: formatDuration(now - lastSeenMs),
        online: formatDuration(now - firstSeenMs),
      };
    },
    [formatDuration]
  );

  const copyToClipboard = useCallback(
    async (text: string) => {
      try {
        if (!text) return;
        await navigator.clipboard.writeText(text);
        toast.success(t("Online.copied"));
      } catch (e: any) {
        toast.error(String(e));
      }
    },
    [t, toast]
  );

  const pasteJoinRoomCode = useCallback(async () => {
    try {
      const text = await navigator.clipboard.readText();
      if (!String(text || "").trim()) return;
      setJoinRoomCode(text);
    } catch (e: any) {
      toast.error(String(e));
    }
  }, [toast]);

  const stopAll = useCallback(async () => {
    try {
      if (heartbeatTimerRef.current) {
        window.clearInterval(heartbeatTimerRef.current);
        heartbeatTimerRef.current = null;
      }
      if (peersTimerRef.current) {
        window.clearInterval(peersTimerRef.current);
        peersTimerRef.current = null;
      }
      await invoke("paperconnect_server_stop").catch(() => undefined);
      await invoke("easytier_stop").catch(() => undefined);
    } finally {
      setRunning(false);
      setRunningRole(null);
      hostIdentityRef.current = null;
      setLatencyMs(null);
      setCenter(null);
      setPlayers([]);
      setPeers([]);
      setGameEndpoint(null);
      setActiveRoom(null);
      setHostRoom(null);
      try {
        localStorage.removeItem(HOST_ROOM_STORAGE_KEY);
        localStorage.removeItem(ACTIVE_ROOM_STORAGE_KEY);
      } catch {
        // ignore
      }
      appendStatus(t("Online.stopped"));
    }
  }, [appendStatus, t]);

  const refreshPeers = useCallback(async () => {
    try {
      const list = (await invoke("easytier_embedded_peers")) as EasyTierPeer[];
      setPeers(Array.isArray(list) ? list : []);
    } catch {
      // ignore (avoid toast spam on background refresh)
    }
  }, []);

  const refreshHostPlayers = useCallback(async () => {
    try {
      const snap = (await invoke("paperconnect_server_state")) as any;
      const list: any[] = Array.isArray(snap?.players) ? snap.players : [];
      setPlayers(
        list.map((p) => ({
          player: String(p?.player || ""),
          clientId: String(p?.clientId || ""),
          isRoomHost: Boolean(p?.isRoomHost),
          lastSeenMs: Number(p?.lastSeenMs || 0),
          firstSeenMs: Number(p?.firstSeenMs || 0),
          returnTime: Number(snap?.returnTime || 0),
        }))
      );
    } catch {
      // ignore
    }
  }, []);

  const checkVirtualIpHintOnce = useCallback(async () => {
    try {
      const st = (await invoke("easytier_embedded_status")) as EasyTierEmbeddedStatus | null;
      if (st && !String(st?.ipv4 || "").trim()) {
        appendStatus(t("Online.no_virtual_ip_hint"));
      }
    } catch {
      // ignore
    }
  }, [appendStatus, t]);

  const waitForVirtualIp = useCallback(
    async (maxWaitMs: number): Promise<string | null> => {
      const deadline = Date.now() + Math.max(0, Number(maxWaitMs || 0));
      while (Date.now() < deadline) {
        try {
          const st = (await invoke("easytier_embedded_status")) as EasyTierEmbeddedStatus | null;
          const ip = String(st?.ipv4 || "").trim();
          if (ip) return ip;
        } catch {
          // ignore
        }
        await new Promise((r) => window.setTimeout(r, 500));
      }
      return null;
    },
    []
  );

  const hostHeartbeatOnce = useCallback(async () => {
    const frozen = hostIdentityRef.current;
    const hostPlayerName = frozen?.playerName || (playerName.trim() ? playerName.trim() : "host");
    const hostClientId = frozen?.clientId || (clientId.trim() ? clientId.trim() : "BMCBL");
    await invoke("paperconnect_tcp_request", {
      host: "127.0.0.1",
      port: pcPort,
      proto: "c:player",
      body: { clientId: hostClientId, playerName: hostPlayerName },
    }).catch(() => undefined);
    await refreshHostPlayers();
  }, [clientId, pcPort, playerName, refreshHostPlayers]);

  const startHost = useCallback(async () => {
    setStatusText("");
    try {
      const pickedPort = Number(await invoke("paperconnect_pick_listen_port"));
      if (!pickedPort) throw new Error("failed to pick listen port");
      setPcPort(pickedPort);
      const hostname = `paper-connect-server-${pickedPort}`;

      const nextRoom = (await invoke("paperconnect_generate_room")) as PaperConnectRoom;
      setHostRoom(nextRoom);
      setActiveRoom(nextRoom);
      appendStatus(`Room: ${nextRoom.roomCode}`);
      setRunningRole("host");

      const hostPlayerName = playerName.trim() ? playerName.trim() : "host";
      const hostClientId = clientId.trim() ? clientId.trim() : "BMCBL";
      hostIdentityRef.current = { playerName: hostPlayerName, clientId: hostClientId };
      appendStatus(t("Online.starting_easytier"));
      await invoke("easytier_start", {
        networkName: nextRoom.networkName,
        networkSecret: nextRoom.networkSecret,
        peers: parsePeers(bootstrapPeer),
        hostname,
        options: {
          disableP2p: disableP2P,
          noTun,
          tcpWhitelist: [pickedPort],
          udpWhitelist: gamePorts,
          ipv4: noTun ? "10.144.144.1" : null,
        },
      });
      await checkVirtualIpHintOnce();
      const hostVip = await waitForVirtualIp(20_000);
      if (hostVip) appendStatus(`Virtual IP: ${hostVip}`);
      appendStatus(t("Online.starting_paperconnect_server"));
      await invoke("paperconnect_server_start", {
        args: {
          listenPort: pickedPort,
          gamePort: primaryGamePort,
          gameType: "MinecraftBedrock",
          gameProtocolType: "UDP",
          roomHostPlayerName: hostPlayerName,
          roomHostClientId: hostClientId,
        },
      });
      setRunning(true);

      await hostHeartbeatOnce();
      await refreshPeers();
      if (peersTimerRef.current) window.clearInterval(peersTimerRef.current);
      peersTimerRef.current = window.setInterval(() => {
        refreshPeers().catch(() => undefined);
      }, 2500);

      if (heartbeatTimerRef.current) window.clearInterval(heartbeatTimerRef.current);
      heartbeatTimerRef.current = window.setInterval(() => {
        hostHeartbeatOnce().catch(() => undefined);
      }, 5000);

      appendStatus(t("Online.host_ready"));
    } catch (e: any) {
      toast.error(String(e));
      setHostRoom(null);
      await stopAll();
    }
  }, [appendStatus, bootstrapPeer, checkVirtualIpHintOnce, clientId, disableP2P, gamePorts, hostHeartbeatOnce, noTun, parsePeers, playerName, primaryGamePort, refreshPeers, stopAll, t, toast]);

  const discoverCenter = useCallback(async (): Promise<PaperConnectCenter | null> => {
    try {
      const c = (await invoke("paperconnect_find_center", {})) as PaperConnectCenter | null;
      setCenter(c);
      if (!c) appendStatus(t("Online.center_not_found"));
      if (c && !String(c?.ipv4 || "").trim()) appendStatus(t("Online.center_no_ipv4"));
      return c;
    } catch (e: any) {
      toast.error(String(e));
      return null;
    }
  }, [appendStatus, t, toast]);

  const pingCenter = useCallback(
    async (c: PaperConnectCenter): Promise<number> => {
      const host = String(c?.ipv4 || "").trim();
      if (!host) throw new Error(t("Online.err_center_no_ipv4"));
      const body = { time: Date.now() };
      const start = performance.now();
      const resp = (await invoke("paperconnect_tcp_request", {
        host,
        port: c.port,
        proto: "c:ping",
        body,
      })) as any;
      setLatencyMs(Math.round(performance.now() - start));
      const nextPort = Number(resp?.gamePort || 0);
      if (!nextPort) throw new Error(t("Online.err_bad_ping_resp"));
      setGameEndpoint({ ip: host, port: nextPort });
      appendStatus(`${t("Online.game_endpoint")}: ${host}:${nextPort}`);
      return nextPort;
    },
    [appendStatus, t]
  );

  const playerHeartbeatOnce = useCallback(
    async (c: PaperConnectCenter) => {
      const host = String(c?.ipv4 || "").trim();
      if (!host) throw new Error(t("Online.err_center_no_ipv4"));
      const effectiveClientId = String(clientId || "").trim() || "BMCBL";
      let effectivePlayerName = normalizePlayerName(playerName);
      if (!effectivePlayerName) {
        effectivePlayerName = generateDefaultPlayerName();
        setPlayerName(effectivePlayerName);
      }
      const body = { clientId: effectiveClientId, playerName: effectivePlayerName };
      const start = performance.now();
      const resp = (await invoke("paperconnect_tcp_request", {
        host,
        port: c.port,
        proto: "c:player",
        body,
      })) as any;
      setLatencyMs(Math.round(performance.now() - start));
      const list: any[] = Array.isArray(resp?.players) ? resp.players : [];
      setPlayers(
        list.map((p) => ({
          player: String(p?.player ?? p?.playerName ?? p?.player_name ?? ""),
          clientId: String(p?.clientId ?? p?.client_id ?? ""),
          isRoomHost: (p?.isRoomHost ?? p?.is_room_host) === true || (p?.isRoomHost ?? p?.is_room_host) === 1 || (p?.isRoomHost ?? p?.is_room_host) === "true",
          lastSeenMs: Number(p?.lastSeenMs ?? p?.last_seen_ms ?? 0),
          firstSeenMs: Number(p?.firstSeenMs ?? p?.first_seen_ms ?? 0),
          returnTime: Number(resp?.returnTime ?? resp?.return_time ?? 0),
        }))
      );
    },
    [clientId, playerName, t]
  );

  const startJoin = useCallback(async () => {
    if (!joinRoomCode.trim()) return toast.error(t("Online.err_need_room_code"));
    setStatusText("");

    try {
      const parsed = (await invoke("paperconnect_parse_room_code", { roomCode: joinRoomCode })) as PaperConnectRoom;
      setActiveRoom(parsed);
      setRunningRole("join");

      appendStatus(t("Online.starting_easytier"));
      await invoke("easytier_start", {
        networkName: parsed.networkName,
        networkSecret: parsed.networkSecret,
        peers: parsePeers(bootstrapPeer),
        hostname: null,
        options: { disableP2p: disableP2P, noTun },
      });
      await checkVirtualIpHintOnce();
      if (!noTun) {
        await waitForVirtualIp(20_000);
      }
      setRunning(true);

      appendStatus(t("Online.discovering_center"));
      let c: PaperConnectCenter | null = null;
      const deadline = Date.now() + 60_000;
      while (Date.now() < deadline) {
        const next = (await invoke("paperconnect_find_center", {})) as PaperConnectCenter | null;
        if (next) {
          setCenter(next);
          if (String(next?.ipv4 || "").trim()) {
            c = next;
            break;
          }
        }
        await new Promise((r) => window.setTimeout(r, 1200));
      }
      if (!c) {
        appendStatus(t("Online.center_not_found"));
        appendStatus(t("Online.center_retry_hint"));
        return;
      }

      const useNoTun = Boolean(noTun);
      const cForRequest: PaperConnectCenter = useNoTun
        ? { ...c, ipv4: "127.0.0.1" }
        : c;

      if (useNoTun) {
        appendStatus("Setting up port forward...");
        let udpPorts = [primaryGamePort, DEFAULT_JOIN_UDP_PORT_FALLBACK]
          .filter((p) => Number.isFinite(p) && p > 0 && p <= 65535);
        udpPorts = Array.from(new Set(udpPorts));
        await invoke("easytier_restart_with_port_forwards", {
          forwards: [
            { proto: "tcp", bindPort: c.port, dstIp: c.ipv4, dstPort: c.port },
            ...udpPorts.map((p) => ({ proto: "udp", bindPort: p, dstIp: c.ipv4, dstPort: p })),
          ],
        });
      }

      appendStatus(t("Online.pinging_center"));
      const pingDeadline = Date.now() + 30_000;
      let gamePortFromPing = 0;
      while (true) {
        try {
          gamePortFromPing = await pingCenter(cForRequest);
          break;
        } catch (e: any) {
          const msg = String(e || "");
          if (Date.now() >= pingDeadline) throw e;
          if (msg) appendStatus(`${t("Online.ping_retry")}: ${msg}`);
          await new Promise((r) => window.setTimeout(r, 1200));
        }
      }

      if (useNoTun && gamePortFromPing > 0 && gamePortFromPing !== primaryGamePort && gamePortFromPing !== DEFAULT_JOIN_UDP_PORT_FALLBACK) {
        appendStatus(`Updating port forward for game port ${gamePortFromPing}...`);
        const udpPorts = Array.from(new Set([primaryGamePort, DEFAULT_JOIN_UDP_PORT_FALLBACK, gamePortFromPing]));
        await invoke("easytier_restart_with_port_forwards", {
          forwards: [
            { proto: "tcp", bindPort: c.port, dstIp: c.ipv4, dstPort: c.port },
            ...udpPorts.map((p) => ({ proto: "udp", bindPort: p, dstIp: c.ipv4, dstPort: p })),
          ],
        });
      }

      appendStatus(t("Online.join_ready"));

      if (heartbeatTimerRef.current) window.clearInterval(heartbeatTimerRef.current);
      heartbeatTimerRef.current = window.setInterval(() => {
        playerHeartbeatOnce(cForRequest).catch(() => undefined);
      }, 5000);
      try {
        await playerHeartbeatOnce(cForRequest);
      } catch (e: any) {
        // Don't stop the whole session if the first heartbeat fails; background retry may succeed
        // once the overlay/port-forward is fully established.
        const msg = String(e || "");
        if (msg) appendStatus(msg);
      }

      await refreshPeers();
      if (peersTimerRef.current) window.clearInterval(peersTimerRef.current);
      peersTimerRef.current = window.setInterval(() => {
        refreshPeers().catch(() => undefined);
      }, 2500);
    } catch (e: any) {
      toast.error(String(e));
      await stopAll();
    }
  }, [appendStatus, bootstrapPeer, checkVirtualIpHintOnce, disableP2P, joinRoomCode, noTun, parsePeers, pingCenter, playerHeartbeatOnce, primaryGamePort, refreshPeers, stopAll, t, toast]);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      const st = (await invoke("easytier_embedded_status").catch(() => null)) as EasyTierEmbeddedStatus | null;
      if (cancelled) return;
      if (!st) {
        setRunning(false);
        setRunningRole(null);
        setLatencyMs(null);
        setCenter(null);
        setPlayers([]);
        setPeers([]);
        setGameEndpoint(null);
        setActiveRoom(null);
        setHostRoom(null);
        return;
      }

      setRunning(true);

      const hostSnap = (await invoke("paperconnect_server_state").catch(() => null)) as any;
      if (cancelled) return;

      if (hostSnap) {
        setRunningRole("host");
        const lp = Number(hostSnap?.listenPort || hostSnap?.listen_port || 0);
        if (lp > 0) setPcPort(lp);
        await refreshHostPlayers();
        await refreshPeers();

        if (peersTimerRef.current) window.clearInterval(peersTimerRef.current);
        peersTimerRef.current = window.setInterval(() => {
          refreshPeers().catch(() => undefined);
        }, 2500);

        if (heartbeatTimerRef.current) window.clearInterval(heartbeatTimerRef.current);
        heartbeatTimerRef.current = window.setInterval(() => {
          refreshHostPlayers().catch(() => undefined);
        }, 5000);
        return;
      }

      setRunningRole("join");
      const c = (await invoke("paperconnect_find_center", {}).catch(() => null)) as PaperConnectCenter | null;
      if (cancelled || !c) return;

      setCenter(c);
      const cForRequest: PaperConnectCenter = noTun ? { ...c, ipv4: "127.0.0.1" } : c;

      await pingCenter(cForRequest).catch(() => undefined);
      await playerHeartbeatOnce(cForRequest).catch(() => undefined);
      await refreshPeers();

      if (peersTimerRef.current) window.clearInterval(peersTimerRef.current);
      peersTimerRef.current = window.setInterval(() => {
        refreshPeers().catch(() => undefined);
      }, 2500);

      if (heartbeatTimerRef.current) window.clearInterval(heartbeatTimerRef.current);
      heartbeatTimerRef.current = window.setInterval(() => {
        playerHeartbeatOnce(cForRequest).catch(() => undefined);
      }, 5000);
    })();

    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    return () => {
      if (heartbeatTimerRef.current) window.clearInterval(heartbeatTimerRef.current);
      if (peersTimerRef.current) window.clearInterval(peersTimerRef.current);
    };
  }, []);

  const easyTierSettingsDirty =
    bootstrapPeerDraft !== bootstrapPeer || disableP2PDraft !== disableP2P || noTunDraft !== noTun;

  return (
    <>
      <div className="online-layout">
        <div className="online-left">
          <div className="glass online-card">
            <div className="online-notice">{t("Online.notice")}</div>
            <div className="online-actions" style={{ marginTop: 12 }}>
              <button className="online-btn" onClick={openEasyTierSettings} disabled={running}>
                <SettingsIcon size={18} />
                EasyTier {t("Sidebar.settings")}
              </button>
              <span className={running ? "online-pill online-pill--running" : "online-pill"}>
                {running ? t("Online.state_running") : t("Online.state_stopped")}
              </span>
              {running && latencyMs !== null && (
                <span className="online-pill">
                  {t("Online.latency")}: {latencyMs}ms
                </span>
              )}
              <button className="online-btn online-btn--danger" onClick={stopAll} disabled={!running}>
                {t("Online.stop")}
              </button>
            </div>
          </div>

          <div className="glass online-card">
            <div className="online-section-title">{t("Online.mode_join")}</div>
            <ol className="online-steps">
              <li>{t("Online.join_step_1")}</li>
              <li>{t("Online.join_step_2")}</li>
            </ol>

            <div className="online-field" style={{ marginTop: 12 }}>
              <div className="online-label">{t("Online.room_code")}</div>
              <div className="online-control online-control--row">
                <input
                  className="online-input online-mono"
                  value={joinRoomCode}
                  onChange={(e) => setJoinRoomCode(e.target.value)}
                  placeholder="P/XXXX-XXXX-XXXX-XXXX"
                  disabled={running}
                />
                <button className="online-btn" onClick={() => setJoinRoomCode("")} disabled={running || !joinRoomCode}>
                  {t("Online.clear")}
                </button>
                <button className="online-btn" onClick={pasteJoinRoomCode} disabled={running}>
                  {t("Online.paste")}
                </button>
                <button className="online-btn online-btn--primary" onClick={startJoin} disabled={running}>
                  {t("Online.start_join")}
                </button>
              </div>
            </div>

            <details className="online-details" style={{ marginTop: 12 }}>
              <summary className="online-details-summary">{t("Online.advanced")}</summary>
              <div className="online-details-body">
                <div className="online-field" style={{ marginTop: 12 }}>
                  <div className="online-label">{t("Online.player_name")}</div>
                  <div className="online-control">
                    <input
                      className="online-input"
                      value={playerName}
                      onChange={(e) => setPlayerName(e.target.value)}
                      placeholder={t("Online.player_name_placeholder")}
                      disabled={running}
                    />
                  </div>
                </div>

                <div className="online-field" style={{ marginTop: 12 }}>
                  <div className="online-label">{t("Online.client_id")}</div>
                  <div className="online-control">
                    <input
                      className="online-input online-mono"
                      value={clientId}
                      readOnly
                      disabled
                    />
                  </div>
                </div>
              </div>
            </details>
          </div>

          <div className="glass online-card">
            <div className="online-section-title">{t("Online.mode_host")}</div>
            <ol className="online-steps">
              <li>{t("Online.host_step_1")}</li>
              <li>{t("Online.host_step_2")}</li>
              <li>{t("Online.host_step_3")}</li>
            </ol>

            {hostnameForHost && (
              <div className="online-field" style={{ marginTop: 12 }}>
                <div className="online-label">{t("Online.hostname")}</div>
                <div className="online-control online-control--row">
                  <input className="online-input online-mono" value={hostnameForHost} readOnly disabled />
                </div>
              </div>
            )}

            <details className="online-details" style={{ marginTop: 12 }}>
              <summary className="online-details-summary">{t("Online.advanced")}</summary>
              <div className="online-details-body">
                <div className="online-field" style={{ marginTop: 12 }}>
                  <div className="online-label">{t("Online.open_ports")}</div>
                  <div className="online-control">
                    <input
                      className="online-input online-mono"
                      value={gamePortsText}
                      onChange={(e) => setGamePortsText(e.target.value)}
                      onBlur={() => setGamePortsText((v) => normalizePortListText(v, DEFAULT_GAME_PORTS))}
                      placeholder={t("Online.open_ports_placeholder")}
                      disabled={running}
                    />
                    <div className="online-inline-hint">{t("Online.open_ports_hint")}</div>
                  </div>
                </div>

                <div className="online-field" style={{ marginTop: 12 }}>
                  <div className="online-label">{t("Online.player_name")}</div>
                  <div className="online-control">
                    <input
                      className="online-input"
                      value={playerName}
                      onChange={(e) => setPlayerName(e.target.value)}
                      placeholder={t("Online.player_name_placeholder")}
                      disabled={running}
                    />
                  </div>
                </div>

                <div className="online-field" style={{ marginTop: 12 }}>
                  <div className="online-label">{t("Online.client_id")}</div>
                  <div className="online-control">
                    <input
                      className="online-input online-mono"
                      value={clientId}
                      readOnly
                      disabled
                    />
                  </div>
                </div>
              </div>
            </details>

            <div className="online-actions" style={{ marginTop: 14 }}>
              <button className="online-btn online-btn--primary" onClick={startHost} disabled={running}>
                {t("Online.start_host")}
              </button>
            </div>

            {hostRoom && (
              <div className="online-kv" style={{ marginTop: 12 }}>
                <div>
                  <div className="online-k">{t("Online.room_code")}</div>
                  <div className="online-v online-mono online-v--row">
                    <span>{hostRoom.roomCode}</span>
                    <button className="online-btn" onClick={() => copyToClipboard(hostRoom.roomCode)}>
                      {t("Online.copy")}
                    </button>
                  </div>
                </div>
              </div>
            )}
          </div>

          {runningRole && (center || gameEndpoint || players.length > 0 || peers.length > 0 || activeRoom) && (
            <div className="glass online-card">
              <div className="online-section-title">{t("Online.room_status")}</div>
              <div className="online-kv" style={{ marginTop: 12 }}>
                {activeRoom && (
                  <div>
                    <div className="online-k">{t("Online.room_code")}</div>
                    <div className="online-v online-mono">{activeRoom.roomCode}</div>
                  </div>
                )}
                {center && (
                  <div>
                    <div className="online-k">{t("Online.center")}</div>
                    <div className="online-v online-mono">
                      {center.hostname} ({String(center.ipv4 || "-")}:{center.port})
                    </div>
                  </div>
                )}
                {running && latencyMs !== null && (
                  <div>
                    <div className="online-k">{t("Online.latency")}</div>
                    <div className="online-v online-mono">{latencyMs}ms</div>
                  </div>
                )}
                {gameEndpoint && (
                  <div>
                    <div className="online-k">{t("Online.game_endpoint")}</div>
                    <div className="online-v online-mono online-v--row">
                      <span>
                        {gameEndpoint.ip}:{gameEndpoint.port}
                      </span>
                      <button className="online-btn" onClick={() => copyToClipboard(`${gameEndpoint.ip}:${gameEndpoint.port}`)}>
                        {t("Online.copy")}
                      </button>
                    </div>
                    <div className="online-subhint">{t("Online.game_endpoint_help")}</div>
                  </div>
                )}
                {!!players.length && (
                  <div>
                    <div className="online-k">{t("Online.players")}</div>
                    <div className="online-players">
                      {players.map((p) => (
                        <div key={`${p.clientId}:${p.player}`} className="online-player">
                          <span className={p.isRoomHost ? "online-badge online-badge--host" : "online-badge"}>
                            {p.isRoomHost ? "HOST" : "MEM"}
                          </span>
                          <span className="online-player-name">{p.player}</span>
                          <span className="online-player-id online-mono">
                            <span>{p.clientId}</span>
                            {(() => {
                              const meta = calcHeartbeatText(p.returnTime ?? null, p);
                              if (!meta) return null;
                              return (
                                <span className="online-player-meta">
                                  <span>
                                    {t("Online.last_heartbeat")}: {meta.last}
                                  </span>
                                  <span>
                                    {t("Online.online_for")}: {meta.online}
                                  </span>
                                </span>
                              );
                            })()}
                          </span>
                        </div>
                      ))}
                    </div>
                  </div>
                )}
                {!!peers.length && (
                  <div>
                    <div className="online-k">{t("Online.peers")}</div>
                    <div className="online-players">
                      {peers.map((p) => (
                        <div key={`${p.hostname}:${p.ipv4 || ""}`} className="online-player">
                          <span className="online-badge">NODE</span>
                          <span className="online-player-name">{p.hostname || "-"}</span>
                          <span className="online-player-id online-mono">{String(p.ipv4 || "-")}</span>
                        </div>
                      ))}
                    </div>
                  </div>
                )}
              </div>
            </div>
          )}
        </div>

        <div className="online-right">
          <div className="glass online-card">
            <div className="online-log-header">
              <div className="online-section-title">{t("Online.log")}</div>
              <div className="online-log-actions">
                <button className="online-btn" onClick={() => setStatusText("")} disabled={!statusText}>
                  {t("Online.clear_log")}
                </button>
              </div>
            </div>
            <pre className="online-log">{statusText || " "}</pre>
          </div>
        </div>
      </div>
      {easyTierSettingsOpen
        ? createPortal(
            <div
              className="et-modal-overlay"
              role="dialog"
              aria-modal="true"
              onClick={closeEasyTierSettings}
            >
              <div className="et-modal" onClick={(e) => e.stopPropagation()}>
                <div className="et-header">
                  <h3 className="et-title">EasyTier {t("Sidebar.settings")}</h3>
                  <button className="et-icon-btn" onClick={closeEasyTierSettings} title={t("common.cancel")}>
                    <X size={18} />
                  </button>
                </div>

                <div className="et-body">
                  <div className="online-field">
                    <div className="online-label">{t("Online.bootstrap_peer")}</div>
                    <div className="online-control">
                      <textarea
                        className="online-input online-mono online-textarea"
                        value={bootstrapPeerDraft}
                        onChange={(e) => setBootstrapPeerDraft(e.target.value)}
                        placeholder={t("Online.bootstrap_peer_placeholder")}
                        rows={2}
                      />
                    </div>
                  </div>

                  <div className="online-field" style={{ marginTop: 12 }}>
                    <div className="online-label">{t("Online.easytier_flags")}</div>
                    <div className="online-control online-control--row">
                      <label className="online-check">
                        <input
                          type="checkbox"
                          checked={disableP2PDraft}
                          onChange={(e) => setDisableP2PDraft(e.target.checked)}
                        />
                        <span>{t("Online.disable_p2p")}</span>
                      </label>
                      <label className="online-check">
                        <input type="checkbox" checked={noTunDraft} onChange={(e) => setNoTunDraft(e.target.checked)} />
                        <span>{t("Online.no_tun")}</span>
                      </label>
                    </div>
                  </div>

                  <div className="online-hint" style={{ marginTop: 12 }}>
                    {t("Online.bootstrap_peer_hint")}
                  </div>
                </div>

                <div className="et-footer">
                  <button className="et-btn et-btn--cancel" onClick={closeEasyTierSettings}>
                    {t("common.cancel")}
                  </button>
                  <button
                    className="et-btn et-btn--primary"
                    onClick={applyEasyTierSettings}
                    disabled={!easyTierSettingsDirty}
                  >
                    {t("common.confirm")}
                  </button>
                </div>
              </div>
            </div>,
            document.body
          )
        : null}
    </>
  );
}
