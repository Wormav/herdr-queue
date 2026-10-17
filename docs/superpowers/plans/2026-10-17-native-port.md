# Herdr Queue Native Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** Livrer Herdr Queue en Rust en conservant les données et comportements existants.

**Architecture:** Modules concrets avec une base SQLite compatible et le même socket JSON.
Interface Ratatui pilotée par événements ; requêtes lentes dans des workers bornés.

**Tech Stack:** Rust 1.88+, Ratatui 0.30.2, Crossterm 0.29, ratatui-textarea 0.9.3, rusqlite 0.32,
serde/serde_json, sha2, libc. Aucun runtime Python en production.

**Spec:** `docs/superpowers/specs/2026-10-17-native-port-design.md`

## Global Constraints

- Identifiants `herdr-drafts`, `open`, `drafts` inchangés ; macOS et Linux.
- Base `drafts.sqlite3` et clés Python compatibles ; aucune suppression de données.
- 65 536 octets UTF-8 par message ; aucun contrôle hors LF/TAB.
- Raccourcis Cmd existants, Entrée pour ajouter, souris, couleur native.
- Aucun renvoi automatique ; identité complète vérifiée avant envoi.
- Objectif mémoire : moins de 20 Mio RSS au repos, résultat effectivement mesuré.
- Commentaires et docstrings du code en anglais ; interface française.

## Review Focus

- Les clés d'une ancienne base contenant Unicode et sessions JSON restent identiques.
- Un envoi dont l'accusé est perdu ne repart jamais par un second clic.
- Coller plusieurs lignes ne déclenche pas plusieurs ajouts ou envois.
- Fermer pendant une requête ne perd pas le composeur et conserve l'envoi incertain.
- Une installation sans compilateur reçoit le bon binaire et vérifie son empreinte.

---

### Task 1: Modèles et stockage compatible

**Files:** `Cargo.toml`, `Cargo.lock`, `native/src/{lib,models,store}.rs`, tests de stockage.

**Interfaces:** `Target` expose les champs Python, `key()`, `lock_key()`, `from_json()`.
`DraftStore` expose `open`, `composer`, `save_composer`, `commit_composer`, `get`,
`list_drafts`, `claim`, `finish`, `resolve`, `recover_inflight`, `delete`, `move_draft`,
`recorded_target`, `resolve_target`, `adopt_session`. `PanelLock::acquire` libère par Drop.
Tous les résultats utilisent `models::Result<T>` ; méthodes de stockage sur `&self`.

- [x] Écrire les tests de clés Python littérales, reprise SQLite, transactions et verrouillage.
- [x] Observer leur échec, implémenter les modèles et le stockage, vérifier les tests.
- [x] Vérifier la lecture d'une base réellement produite par la version Python.

### Task 2: Transport Herdr et règles métier

**Files:** `native/src/{herdr,discovery,service}.rs`, tests socket et service.

**Interfaces:** `HerdrClient::new(path)`, `with_timeout(Duration)`, `request`, `get_agent`,
`list_agents`, `prompt`; client clonable. `discover_sessions(environment, session, socket)`
et `load_agents(sessions)` renvoient agents et erreurs. `DraftService` possède `store`,
`client`, `target`, avec `new`, `demo`, `save`, `refresh`, `verify_target`, `send`.

- [x] Écrire les tests de réponse fragmentée, refus connu, réponse perdue et cible changée.
- [x] Observer les échecs puis implémenter transport et règles sans processus CLI par envoi.
- [x] Vérifier les bornes, délais et transitions à partir de sockets locales de test.

### Task 3: Interface native et interaction

**Files:** `native/src/ui.rs` et éventuels sous-modules de rendu/événements.

**Interfaces:** `ui::run_queue(service, panel_id, allow_switch) -> Result<bool>` retourne
vrai pour changer d'agent. `ui::pick_agent(environment, session, socket, notice)` retourne
`Result<Option<Agent>>`. Utilise les modules concrets des tâches 1 et 2.

- [x] Écrire les tests des événements, sauvegardes, envoi suivant et rendu TestBackend.
- [x] Observer les échecs, implémenter édition Unicode, souris, confirmations et historique.
- [x] Vérifier le protocole clavier Kitty, le collage et les petits terminaux en PTY.

### Task 4: Lancement et distribution

**Files:** `native/src/{entry,main}.rs`, `run.sh`, `install.sh`, `herdr-plugin.toml`,
`.github/workflows/*.yml`, `README.md`, `docs/TESTING.md`.

**Interfaces:** Mêmes commandes et variables que la version actuelle ; binaire `herdr-queue`.
Les archives publiées contiennent ce binaire et leurs empreintes SHA-256.

- [x] Tester commandes, ouverture du panneau et erreurs de contexte.
- [x] Implémenter les entrées, le lanceur, l'installation et les builds natifs.
- [x] Retirer Python du chemin de production et adapter la documentation anglaise.

### Task 5: Validation et mesure

**Files:** tests d'intégration, procédure de benchmark et résultats documentés.

- [x] Exécuter `cargo test`, `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`.
- [x] Tester une base Python existante et le démarrage du binaire sans Python/uv.
- [x] Construire en release et comparer le pic RSS sur la même démo 120×40 de sept secondes.
- [ ] Effectuer une revue indépendante puis corriger les défauts importants avant livraison.
