# Herdr Drafts — plan d'implémentation

**Goal:** fournir le panneau de brouillons validé, prêt pour un test commun dans Herdr.
**Architecture:** interface Textual, service d'envoi concret, client socket et stockage SQLite.
**Tech Stack:** Python >=3.11, Textual >=6.6,<7, SQLite, Herdr >=0.9.3, pytest, Ruff, mypy.
**Spec:** `docs/superpowers/specs/2026-10-17-herdr-drafts-design.md`.
**Exécution:** directe et autonome, conformément à la demande de l'utilisateur.

## Contraintes globales

Texte utilisateur en français ; commentaires de code en anglais. Envoi manuel uniquement.
Aucune écriture dans les conversations existantes pendant la validation. Aucun prompt dans
une commande shell. Conservation du texte en cas de refus ou d'incertitude.

## Points de revue

Identité remplacée dans le même panneau ; double clic ; arrêt pendant envoi ; Unicode et
contrôles de terminal ; deux instances sur une même file.

## Tâches

1. [x] **Stockage** — tests rouges puis modèles et transactions : création, édition,
   suppression, ordre, compositeur, historique, états d'envoi et reprise. Fichiers
   `models.py`, `store.py`, `tests/test_store.py`.
2. [x] **API et envoi** — tests avec socket factice, puis client borné et service : bon
   destinataire, prompt littéral, refus, incertitude et concurrence. Fichiers `herdr.py`,
   `service.py`, `tests/test_herdr.py`, `tests/test_service.py`.
3. [x] **Interface** — tests Pilot rouges puis édition, cartes cliquables, confirmations,
   historique, sauvegarde et raccourcis. Fichiers `app.py`, `app.tcss`, `tests/test_app.py`.
4. [x] **Distribution** — lanceur, manifeste, démonstration, diagnostic et documentation.
   Tests du lanceur, démo terminal et parcours de test commun. Fichiers `__main__.py`, `run.sh`,
   `herdr-plugin.toml`, `docs/TESTING.md`, `README.md`.
5. [x] **Livraison** — Ruff, formatage, mypy, pytest, build, rendu terminal et revue finale.

Chaque tâche commence par une vérification qui échoue pour le comportement manquant et finit
par ses tests ciblés. L'état d'exécution et les décisions restent dans `docs/progress.md`.


## Validation restante avec l’utilisateur

Le lancement dans une vraie session Herdr et le comportement propre à chaque CLI seront
vérifiés ensemble. Le sandbox interdit la création de sockets nommés ; aucun serveur Herdr
ni agent réel n’a été lancé ou contacté pendant le développement.
