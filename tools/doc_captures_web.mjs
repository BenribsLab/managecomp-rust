// Captures web de la documentation : ManageComp Arbitre (téléphone, en
// portrait), écran de piste (ManageComp Live) et pages de la version en
// ligne (organisation, signalements, codes des arbitres, nouvelle
// compétition), avec le serveur local (docker compose).
//
// Crée un club de démonstration, y importe les compétitions d'exemple
// (tools/doc_captures.sh les fabrique) et des arbitres, prépare quelques
// signalements, pilote Edge sans fenêtre par son protocole DevTools (clics,
// connexion), puis retire le club.
//
// Utilisation :  node tools/doc_captures_web.mjs

import { spawn } from "node:child_process";
import { readFileSync, writeFileSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const WEB = join(ROOT, "..", "Bellepoule Web");
const OUT = join(WEB, "site", "static", "docs");
const SAMPLES = join(ROOT, "docs-captures", "samples");
const SERVER = `http://127.0.0.1:${process.env.BP_PORT || 8090}`;
const EDGE = "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe";
const ADMIN = readFileSync(join(WEB, ".env"), "utf8").match(/^BP_ADMIN_KEY=(.*)$/m)[1].trim();
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const enc = encodeURIComponent;

// ------------------------------------------------------------ API

async function api(method, path, { key, json, body } = {}) {
    const headers = {};
    if (key) headers["x-api-key"] = key;
    if (json !== undefined) headers["content-type"] = "application/json";
    const r = await fetch(SERVER + path, { method, headers, body: json !== undefined ? JSON.stringify(json) : body });
    const text = await r.text();
    if (!r.ok) throw new Error(`${method} ${path} : ${r.status} ${text}`);
    return text ? JSON.parse(text) : null;
}

// ------------------------------------------------------------ Edge

class Browser {
    async start() {
        this.profile = mkdtempSync(join(tmpdir(), "managecomp-captures-"));
        this.proc = spawn(EDGE, ["--headless=new", "--disable-gpu", "--hide-scrollbars", "--remote-debugging-port=9333", `--user-data-dir=${this.profile}`, "about:blank"], { stdio: "ignore" });
        let target;
        for (let i = 0; i < 50 && !target; i++) {
            await sleep(200);
            try {
                target = (await (await fetch("http://127.0.0.1:9333/json/list")).json()).find((t) => t.type === "page");
            } catch {}
        }
        if (!target) throw new Error("Edge ne répond pas");
        this.ws = new WebSocket(target.webSocketDebuggerUrl);
        await new Promise((ok) => this.ws.addEventListener("open", ok, { once: true }));
        this.next = 1;
        this.pending = new Map();
        this.ws.addEventListener("message", (ev) => {
            const m = JSON.parse(ev.data);
            if (m.id && this.pending.has(m.id)) {
                this.pending.get(m.id)(m);
                this.pending.delete(m.id);
            }
        });
        await this.send("Page.enable");
    }
    send(method, params = {}) {
        const id = this.next++;
        this.ws.send(JSON.stringify({ id, method, params }));
        return new Promise((ok, ko) => this.pending.set(id, (m) => (m.error ? ko(new Error(`${method} : ${m.error.message}`)) : ok(m.result))));
    }
    async size(width, height, phone) {
        await this.send("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: phone ? 2 : 1, mobile: !!phone });
    }
    async eval(js) {
        const r = await this.send("Runtime.evaluate", { expression: js, awaitPromise: true, returnByValue: true });
        return r.result.value;
    }
    async go(url) {
        await this.send("Page.navigate", { url });
        await sleep(600);
    }
    async wait(selector) {
        for (let i = 0; i < 75; i++) {
            if (await this.eval(`!!document.querySelector(${JSON.stringify(selector)})`)) {
                await sleep(700);
                return;
            }
            await sleep(200);
        }
        throw new Error(`introuvable : ${selector}`);
    }
    async click(selector, text) {
        const ok = await this.eval(`(() => {
            const all = [...document.querySelectorAll(${JSON.stringify(selector)})];
            const el = ${text ? `all.find((e) => e.textContent.includes(${JSON.stringify(text)}))` : "all[0]"};
            if (el) el.click();
            return !!el;
        })()`);
        if (!ok) throw new Error(`rien à cliquer : ${selector} ${text || ""}`);
    }
    async shot(name) {
        const r = await this.send("Page.captureScreenshot", { format: "png" });
        writeFileSync(join(OUT, `${name}.png`), Buffer.from(r.data, "base64"));
        console.log(`ok  ${name}`);
    }
    stop() {
        try { this.ws.close(); } catch {}
        this.proc.kill();
        setTimeout(() => { try { rmSync(this.profile, { recursive: true, force: true }); } catch {} }, 1500);
    }
}

// ------------------------------------------------------------ données

const client = await api("POST", "/api/admin/clients", { key: ADMIN, json: { name: "Club de démonstration" } });
const KEY = client.key;
const b = new Browser();
try {
    for (const f of ["individuel", "jeunes", "equipes"]) {
        await api("POST", `/api/contests/import?name=${f}.cotcot`, { key: KEY, body: readFileSync(join(SAMPLES, `${f}.cotcot`)) });
    }
    // Arbitres de l'organisation (noms fictifs) : QR codes personnels et PIN.
    const referees = "Nom;Prénom;Club\nGARNIER;Sophie;YERRES CE\nCHEVALIER;Marc;MELUN AE\nSANG;Nathalie;CRETEIL\nROUSSEAU;Hugo;MASSY\nLEMAIRE;Camille;YERRES CE\n";
    await api("POST", `/api/organisation/import?weapon=F&name=arbitres.csv`, { key: KEY, body: Buffer.from(referees, "utf8") });
    await api("GET", "/api/organisation", { key: KEY });

    // Arbitres présents, puis un arbitre et une piste (la plus haute) pour la
    // première poule qui le permet.
    let org = await api("GET", "/api/organisation", { key: KEY });
    for (const list of org.referees.filter((l) => l.members.length)) {
        await api("POST", "/api/organisation", { key: KEY, json: { action: "list_attending", weapon: list.weapon, value: true } });
    }
    org = await api("GET", "/api/organisation", { key: KEY });
    const piste = [...org.pistes].sort((a, b) => b.id - a.id)[0];
    const poolJob = org.competitions.flatMap((c) => c.batches.flatMap((b) => b.jobs)).find((j) => j.pool && j.available_referees.length);
    if (poolJob && piste) {
        await api("POST", "/api/organisation", { key: KEY, json: { action: "drop_job", job: poolJob.key, piste: piste.key } });
        await api("POST", "/api/organisation", { key: KEY, json: { action: "drop_referee", referee: poolJob.available_referees[0], piste: piste.key } });
    } else if (piste) {
        // Entraînement des jeunes : un groupe se place en entier.
        for (const c of org.competitions) {
            for (const b of c.batches) {
                const g = (b.group_referees || []).find((x) => x.referees.length);
                if (g) {
                    await api("POST", "/api/organisation", { key: KEY, json: { action: "place_group", competition: c.id, batch: b.id, group: g.group, pistes: [piste.key], referees: [g.referees[0]] } });
                    break;
                }
            }
        }
    }
    const admin = await api("GET", "/api/smartpoule/admin", { key: KEY });
    const who = { c: admin.club, r: 0, k: admin.key };
    const q = `c=${enc(who.c)}&r=0&k=${enc(who.k)}`;
    const jobs = (await api("GET", `/api/smartpoule/phone/jobs?${q}`)).jobs;
    const pool = jobs.find((j) => j.kind === "pool").token;
    const team = jobs.find((j) => j.kind === "match").token;
    const sheet = await api("GET", `/api/smartpoule/phone/sheet?${q}&token=${enc(pool)}`);
    const fencers = sheet.fencers.map((f) => f.reference);
    // Matériel vérifié pour deux tireurs, une absence, un appel du Directoire technique.
    for (const f of fencers.slice(0, 2)) await api("POST", "/api/smartpoule/phone/equipment", { json: { ...who, token: pool, fencer: f, checked: true } });
    await api("POST", "/api/smartpoule/phone/absence", { json: { ...who, token: pool, fencer: fencers[fencers.length - 1], absent: true } });
    // Appel du Directoire technique par un arbitre reconnu par son PIN (poule
    // à laquelle un arbitre est affecté) ; à défaut, par l'administrateur.
    const codes = await api("GET", "/api/organisation/referee-codes", { key: KEY });
    const assigned = jobs.find((j) => j.kind === "pool" && (j.referees || []).some((r) => codes.referees.some((c) => c.reference === r)));
    let caller = { ...who, token: pool };
    if (assigned) {
        const referee = codes.referees.find((c) => assigned.referees.includes(c.reference));
        const session = await api("POST", "/api/smartpoule/phone/pin", { json: { c: who.c, token: assigned.token, pin: referee.pin } });
        caller = { c: session.c, r: session.r, k: session.k, token: assigned.token };
    } else {
        console.log("(aucune poule avec arbitre : appel au nom de l'administrateur)");
    }
    await api("POST", "/api/smartpoule/phone/call", { json: caller });

    await b.start();
    const PHONE = [390, 844, true];

    // ---------------------------------------------- téléphone de l'arbitre
    await b.size(...PHONE);
    await b.go(`${SERVER}/smartpoule/?c=${enc(who.c)}&s=${enc(pool)}`);
    await b.wait(".ref-pin-input");
    await b.shot("arbitre-pin");

    const app = `${SERVER}/smartpoule/?${q}`;
    await b.go(`${app}#/`);
    await b.wait(".ref-card");
    await b.shot("arbitre-liste");
    await b.go(`${app}#/feuille/${enc(pool)}`);
    await b.wait(".ref-grid");
    await b.shot("arbitre-feuille");
    await b.click("td.ref-gname.checked");
    await b.wait(".ref-modal-card");
    await b.shot("arbitre-tireur");
    const bout = sheet.bouts.find((x) => !x.over).index;
    await b.go(`${app}#/match/${enc(pool)}/${bout}`);
    await b.wait(".ref-call");
    await sleep(800);
    await b.shot("arbitre-match");
    await b.go(`${app}#/match/${enc(team)}/0`);
    await b.wait(".ref-call");
    await sleep(800);
    await b.shot("arbitre-relais");

    // ---------------------------------------------- écran de piste
    await api("POST", "/api/smartpoule/phone/live", {
        json: { ...who, piste: 1, competition: "Entraînement des jeunes d'octobre", color: "#3d6fd6", bout: "Groupe 1 · Poule n° 01 · M4",
            red: { name: "MARTIN Lina", score: "3" }, green: { name: "DUBOIS Gabriel", score: "2" }, remaining_ms: 107000, running: false, period: 1, periods: 1, over: false },
    });
    await b.size(1280, 720, false);
    await b.go(`${SERVER}/live/?c=${enc(who.c)}&p=1`);
    await sleep(2500);
    await b.shot("live");

    // ---------------------------------------------- version en ligne
    await b.size(1400, 900, false);
    await b.go(`${SERVER}/`);
    await b.eval(`localStorage.setItem("bellepoule_api_key", ${JSON.stringify(KEY)})`);
    // Même page (seule l'ancre change) : rechargée pour lire la clé.
    await b.send("Page.reload");
    await sleep(600);
    await b.wait(".contest-card");
    await b.click("button", "Nouvelle compétition");
    await b.wait(".new-chooser");
    await b.shot("nouvelle-competition");
    await b.go(`${SERVER}/#/organisation`);
    await b.wait(".reports");
    await b.wait(".nav-dot");
    await b.shot("organisation-signalements");
    await b.click("button", "Arbitres sur téléphone");
    await b.wait(".referee-codes table");
    await b.shot("arbitre-codes");
    await b.size(900, 1200, false);
    await b.click("button", "Imprimer les feuilles personnelles");
    await b.wait(".referee-page");
    await b.shot("arbitre-personnelle");
} finally {
    b.proc && b.stop();
    // Le club de démonstration est retiré (ses compétitions restent sur le disque du conteneur).
    await api("DELETE", `/api/admin/clients/${client.id}`, { key: ADMIN }).catch(() => {});
}
