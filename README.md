# Herdr Drafts

Préparez plusieurs messages pour votre agent Herdr, puis envoyez celui de votre choix.

Lancez l'application dans le terminal de votre choix, sélectionnez un agent Herdr, puis
retrouvez son éditeur multiligne et sa liste de brouillons dans ce même terminal.
Chaque message possède ses boutons **Envoyer**, **Modifier**, **Supprimer**, **↑** et **↓**.
Le compositeur est sauvegardé à chaque changement. Les messages transmis restent dans l'historique.

## Lancer depuis un autre terminal

Prérequis : **macOS ou Linux**, **Herdr 0.9.3+**, **Python 3.11+** et **uv**.
Herdr doit être lancé et sa commande `herdr` disponible dans le PATH.

```sh
/Users/jeremy/Documents/Github/herdr-drafts/run.sh
```

Depuis le dossier du plugin, utilisez simplement `./run.sh`. Le lanceur prépare son
environnement Python au premier démarrage si nécessaire.

La liste affiche le projet, le nom, le type et l'état de chaque agent. Recherchez par nom
ou projet, puis choisissez avec un clic ou **Entrée**. Les brouillons s'ouvrent dans le
terminal de lancement. Le bouton **Agents** ou **Ctrl+G** revient au choix d'agent en
conservant le texte en cours. Chaque conversation garde ses propres brouillons.

La liste s'actualise toutes les cinq secondes et avec **Ctrl+R**. Hors de Herdr, les sessions
locales sont découvertes par `herdr session list --json`. Dans un terminal Herdr, la session
héritée est utilisée. Vous pouvez choisir une session ou un socket précis :

```sh
./run.sh --session default
./run.sh --socket /chemin/vers/herdr.sock
```

Si les brouillons de cet agent sont déjà ouverts ailleurs, fermez cette interface avec
**Ctrl+Q**, puis sélectionnez de nouveau l'agent.

## Essayer la démo

```sh
./run.sh demo
```

La démo fonctionne sans agent ni session Herdr. Ses envois sont simulés et ses données temporaires.

## Ouvrir un panneau dans Herdr

Pour utiliser aussi le panneau intégré à droite de l'agent, liez le plugin depuis son dossier :

```sh
uv sync --frozen --no-dev
herdr plugin link "$PWD"
```

Sélectionnez ensuite le panneau de votre agent dans Herdr et invoquez l'action
**Ouvrir les brouillons**, ou exécutez depuis un terminal dans Herdr :

```sh
herdr plugin action invoke herdr-drafts.open
```

L'action reprend un panneau déjà ouvert pour la même conversation. Le destinataire apparaît
en haut et reste lié au panneau source : changer de focus ne change pas la cible.
Si la conversation est remplacée, fermez son ancien panneau de brouillons avec **Ctrl+Q**,
puis rouvrez l'action. Les textes de l'ancienne conversation restent conservés.

Pour ajouter un raccourci, choisissez une touche libre dans la configuration Herdr :

```toml
[[keys.command]]
key = "prefix+b"
type = "plugin_action"
command = "herdr-drafts.open"
description = "ouvrir les brouillons"
```

Vérifiez avec `herdr config check`, puis rechargez avec `herdr server reload-config`.

## Utilisation

| Action | Souris ou clavier |
| --- | --- |
| Écrire plusieurs lignes | Entrée insère une nouvelle ligne |
| Ajouter ou enregistrer une modification | Bouton principal ou **Ctrl+S** |
| Envoyer un message précis | **Envoyer** sur ce message |
| Préparer un nouveau texte | **Nouveau** ou **Ctrl+N**, avec confirmation si nécessaire |
| Retrouver les messages transmis | **Historique** |
| Réutiliser un ancien message | **Reprendre**, puis ajouter le texte à la liste |
| Choisir un autre agent depuis le terminal autonome | **Agents** ou **Ctrl+G** |
| Actualiser l'état de l'agent | **Ctrl+R** |
| Fermer en conservant les textes | **Ctrl+Q** |

Les boutons sont accessibles avec **Tab** et **Maj+Tab**. La souris fonctionne dans un terminal
qui transmet ses événements à Herdr. Un panneau de **36 colonnes × 24 lignes** convient aux textes
courts ; davantage de place facilite les messages longs.

## Envoi et reprise

Le plugin utilise `agent.prompt`. **Transmis au terminal** signifie que Herdr a confirmé la
soumission du texte ; cela ne prouve pas que l'agent l'a traité. Pendant une exécution, le CLI
décide s'il met le message en attente ou l'utilise pour orienter son travail. Le comportement
de vos versions de Codex, Claude Code et Antigravity reste à vérifier en session réelle.

Un agent bloqué ou dont l'état est inconnu ne reçoit pas de texte. Répondez à sa demande
interactive dans son terminal, puis réessayez. Un refus certain conserve le brouillon.

Après une coupure pendant l'envoi ou un arrêt brutal, le message porte le statut
**Envoi à vérifier**. Regardez dans le terminal, puis choisissez **Déjà transmis** pour le classer,
ou **Remettre en brouillon** après avoir vérifié son absence. Cette dernière action ne l'envoie pas.
Aucun renvoi automatique. Les doubles clics et deux panneaux concurrents ne peuvent pas réclamer
simultanément le même brouillon.

La cible comprend le socket, le terminal, le panneau, l'espace de travail, le type d'agent,
le répertoire et la référence de conversation disponible. La première référence native tardive
est mémorisée pour retrouver les textes après fermeture ou restauration du panneau.
Herdr 0.9.3 ne permet pas de joindre une identité attendue atomiquement à `agent.prompt` : une
petite course reste possible si un autre client remplace l'agent entre vérification et envoi.
Sans intégration native, Herdr peut aussi ne pas reconnaître un remplacement de conversation.

## Données locales

Les textes restent dans `HERDR_PLUGIN_STATE_DIR/drafts.sqlite3`, avec un fichier SQLite privé
et des verrous locaux. Le répertoire par défaut est
`$XDG_STATE_HOME/herdr/plugins/herdr-drafts`, ou `~/.local/state/herdr/plugins/herdr-drafts`.
Le terminal autonome partage ce stockage avec le panneau intégré à Herdr.

```sh
./run.sh doctor
```

Ce diagnostic affiche les versions et le chemin sans contacter une conversation. Aucune
télémétrie, requête de modèle ni service HTTP. Les prompts restent du texte littéral sans
interprétation par un shell. Limite : **64 Kio UTF-8** par message. Les caractères de contrôle
sont refusés, sauf les nouvelles lignes et tabulations.
Pour sauvegarder une base active, utilisez l'API de sauvegarde SQLite ou fermez tous les
panneaux avant de copier le fichier : SQLite peut utiliser des fichiers WAL annexes.

## Développement

```sh
uv sync --frozen
uv run pytest -q
uv run ruff check src tests
uv run ruff format --check src tests
uv run mypy src/herdr_drafts
uv build
```

Les tests couvrent la persistance, les identités tardives et remplacées, les doubles envois,
les coupures, la fragmentation, Unicode, les contrôles de terminal, la découverte des sessions,
le choix d'agent et les interfaces avec Textual Pilot. Ils utilisent de vrais flux Unix
`socketpair` et un faux serveur Herdr, sans modèle
ni contact avec vos sessions existantes.

La création de sockets nommés étant interdite dans l'environnement de développement utilisé,
le lancement dans une session Herdr réelle reste à valider. Voir [le parcours de test](docs/TESTING.md).

## Retirer le plugin

Fermez ses panneaux, puis exécutez `herdr plugin unlink herdr-drafts`.
Votre configuration et vos brouillons restent disponibles. Licence MIT.
