# Test commun depuis un autre terminal

Ce parcours utilise une conversation de test choisie explicitement. Les tests automatisés
n'ont pas validé le comportement de vos versions de CLI pendant leur exécution.

1. Lancer `./run.sh demo`. Ajouter deux messages, les réordonner, modifier le second,
   envoyer uniquement celui-ci, puis le retrouver dans l'historique. Fermer avec ⌘L sur Mac.
2. Lancer un agent de test dans Herdr. Dans un autre terminal, exécuter
   `./run.sh` depuis le checkout, ou le binaire `herdr-queue` téléchargé depuis la release.
   Aucun environnement Python n'est nécessaire. Le premier lancement télécharge le binaire
   natif correspondant à la machine et vérifie son empreinte SHA-256.
3. Vérifier le projet, le nom et l'état dans la liste. Rechercher le projet, puis choisir
   l'agent avec Entrée ou un clic. Vérifier le destinataire au-dessus des brouillons et
   leur affichage dans le terminal de lancement.
4. Pendant que l'agent travaille, préparer trois messages. Vérifier qu'ils restent dans
   le panneau. Utiliser Entrée pour ajouter, et Maj+Entrée pour insérer une nouvelle ligne
   dans un message. Envoyer le deuxième seulement et observer le texte reçu, dont les nouvelles lignes.
   Vérifier ensuite ⌘Y sur Mac : seul le premier message restant doit être transmis.
   Garder un texte dans le compositeur et vérifier qu'il reste présent après cet envoi.
5. Garder un texte inachevé, cliquer sur Agents ou utiliser ⌘O sur Mac, puis choisir un autre
   agent. Revenir au premier : le message restant et le texte inachevé doivent
   être conservés. Fermer avec ⌘L sur Mac, relancer et vérifier de nouveau la restauration.
6. Déclencher une demande interactive dans la conversation de test. Un envoi doit être
   refusé et conservé. Résoudre la demande dans le terminal de l'agent.
7. Refaire les étapes 3–5 avec les autres CLI : Codex, Claude Code et Antigravity.
8. Dans la liste, vérifier ⌘R sur Mac et la mise à jour automatique après le lancement ou
   l'arrêt d'un agent. Une recherche sans résultat doit pouvoir être effacée. Tester
   `./run.sh --session default` si plusieurs sessions Herdr sont présentes.
9. Pour tester aussi le panneau intégré, lier le plugin avec
   `herdr plugin link "$PWD"`, sélectionner un agent
   dans Herdr puis invoquer `herdr-drafts.open` dans la palette d'actions. Vérifier le
   placement à droite. Fermer cette interface avant de choisir le même agent depuis
   le terminal autonome : les brouillons doivent être partagés.

Les touches F2 (ajouter), F8 (suivant) et F10 (fermer) permettent aussi de vérifier le parcours.
La démo simule les transmissions ; seul le parcours en session réelle confirme l'intégration
du terminal, de la souris et de chaque CLI. Aucun changement de raccourci n'est nécessaire.

## Portage natif et mémoire

Les tests automatiques s'exécutent avec `cargo test --locked`. Le formatage et le lint
s'exécutent avec `cargo fmt --check` et `cargo clippy --locked --all-targets -- -D warnings`.
Pour installer les modifications locales : `HERDR_QUEUE_BUILD_FROM_SOURCE=1 sh install.sh`.

Pour vérifier la reprise, fermer l'ancienne version puis ouvrir la nouvelle sur le même
agent : vérifier l'ordre des brouillons, le texte encore en édition, l'historique et les
envois à vérifier. Le fichier SQLite, son schéma et les clés de destination restent compatibles.

Pour mesurer sur macOS, lancer `/usr/bin/time -l ./run.sh demo`, attendre quelques secondes,
puis fermer avec ⌘L. `maximum resident set size` est un pic de mémoire résidente, pas la
taille du binaire. Utiliser le même terminal, la même taille et la même durée pour comparer.
La référence Python mesurée avant portage est 49,64 Mio dans un PTY 120×40, avec deux
brouillons et sept secondes d'ouverture. La mesure ne contacte aucun agent réel.
La comparaison finale et ses limites sont décrites dans [PERFORMANCE.md](PERFORMANCE.md).
