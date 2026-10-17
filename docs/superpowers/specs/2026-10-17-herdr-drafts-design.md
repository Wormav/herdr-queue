# Herdr Drafts

## Besoin validé

Préparer plusieurs prompts pendant qu'un agent CLI travaille dans Herdr. Un panneau terminal
à droite contient un éditeur multiligne et une liste de messages. L'utilisateur déclenche chaque
envoi. Il peut modifier, supprimer et réordonner les brouillons. Les textes restent locaux et
survivent à la fermeture du panneau. Codex, Claude Code et Antigravity utilisent l'API Herdr.
L'utilisateur a demandé le développement autonome, puis une séance de test commune.

## Architecture

Python >=3.11, Textual >=6.6,<7 pour une interface compatible souris, SQLite pour le stockage,
socket Unix JSON de Herdr >=0.9.3. Cible initiale macOS et Linux. Un manifeste et un lanceur
ouvrent le panneau à droite du panneau source. Aucun serveur HTTP ni service permanent.
La fenêtre reste liée à son agent source ; une seconde invocation y revient.

La cible contient le socket, les identifiants de terminal/panneau/espace, le type d'agent et la
référence de conversation disponible. Chaque envoi relit l'agent et vérifie cette identité.
Une référence native apparaissant après le premier prompt peut être adoptée si les autres
identifiants correspondent ; une référence connue qui change bloque l'envoi. Herdr 0.9.3 ne
fournit pas de garde atomique sur l'identité attendue : il subsiste une course entre vérification
et envoi, qui sera documentée sans promettre une isolation parfaite.

## Responsabilités

- `herdr.py` : transport borné, décodage et appels API concrets, sans décision produit.
- `models.py` : types de cible, d'agent et de message.
- `store.py` : transactions locales et conservation des textes.
- `service.py` : validation du destinataire, politique d'envoi et transitions de message.
- `app.py` / `app.tcss` : édition, liste, souris/clavier et affichage des erreurs.
- `__main__.py` : ouverture, lancement, diagnostic et démonstration locale.

## Contrat d'envoi

Un message n'est envoyé qu'au clic explicite. Un agent occupé peut recevoir un prompt ; son
CLI décide quand il le consomme. Un agent bloqué ou inconnu ne reçoit pas de texte.
Une réponse positive signifie « transmis au terminal », pas « exécuté ».
Un refus certain conserve le texte et permet une nouvelle tentative. Une coupure après le
début de soumission produit un état incertain : l'utilisateur vérifie dans le terminal avant de
choisir « déjà transmis » ou « remettre en brouillon ». Aucun renvoi automatique.
Les doubles clics et deux panneaux concurrents ne doivent pas envoyer deux fois le même message.

## Conservation et limites

SQLite dans `HERDR_PLUGIN_STATE_DIR`, sinon un répertoire XDG propre au plugin. Sauvegarde du
compositeur à chaque changement et avant toute navigation/fermeture. Les suppressions demandent
confirmation. Un panneau verrouille sa file pour rendre la reprise après arrêt non ambiguë.
Les prompts sont du texte littéral : aucun passage par un shell. Les contrôles de terminal sont
refusés ; retours à la ligne et tabulations restent permis. Limite initiale 64 Kio UTF-8.

## Validation

Tests de persistance, édition et ordre ; serveur socket factice couvrant Unicode, fragmentation,
erreurs, mauvaise réponse, mauvais destinataire et coupure ; tests Textual Pilot au clavier et
à la souris ; vérification du manifeste et test dans un serveur Herdr isolé avec un faux agent,
sans appel de modèle ni manipulation des sessions existantes. Ruff, mypy, pytest et build.
