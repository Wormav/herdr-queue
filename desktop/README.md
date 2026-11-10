# Herdr Queue pour Desktop

Ce port ajoute un bouton **Queue** à droite de **Changes** dans une copie personnelle de la disposition Herdr. Le bouton ouvre un panneau à droite de l'agent de l'onglet actif. Il conserve les brouillons, l'ordre, le texte en cours et l'historique dans l'état local de Herdr Desktop (`~/.config/herdr-app/shared.json`, clé `herdr-queue.desktop`). L'envoi reste manuel. Un accusé de réception perdu bloque le renvoi jusqu'à ce que l'utilisateur marque le message comme transmis ou le remette en brouillon.

Le port utilise `hc send` pour transmettre le texte au terminal Herdr Core. Le mod exécute donc le programme local `hc` et lit les métadonnées des agents du Desktop. Il ne contacte aucun serveur externe. Les données SQLite de l'extension TUI restent dans `~/.local/state/herdr/plugins/herdr-drafts/drafts.sqlite3` ; elles ne sont pas associées automatiquement aux terminaux du nouveau Core, dont les identifiants diffèrent.

## Installation

```sh
sh desktop/install.sh
herdr-app check ~/.config/herdr-app/mods/herdr-queue
herdr-app check ~/.config/herdr-app/mods/tabs-queue
herdr-app do layout.switch --name main-queue
```

Le fichier `keys.json` de l'utilisateur peut lier `Cmd+I` à `tabs-queue.open_queue` dans la section `layout:main-queue`. Le bouton reste utilisable sans raccourci.

## Contrôles

- **Ajouter à la liste** enregistre le texte en brouillon ; **Envoyer** transmet un seul brouillon à l'agent lié au panneau.
- **Entrée** ajoute le brouillon ; **Maj+Entrée** insère un retour à la ligne.
- **Modifier**, **Supprimer** et les flèches permettent de gérer l'ordre. L'édition préserve le texte qui était en cours de rédaction.
- **Historique** permet de retrouver et réutiliser un message transmis.
- **Envoi à vérifier** apparaît si la connexion est perdue après une tentative d'envoi. Vérifier le terminal avant de choisir **Déjà transmis** ou **Remettre en brouillon**.

Le panneau est lié à l'adresse du terminal Core qui était sélectionné à son ouverture. Un nouveau terminal possède sa propre file. Les brouillons ne sont jamais envoyés automatiquement.

## Vérification

```sh
node --test desktop/tests/queue-state.test.mjs
```
