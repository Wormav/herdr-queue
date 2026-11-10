export const MAX_PROMPT_BYTES = 64 * 1024;

export function promptError(body) {
  if (!body.trim()) return "Écris un message avant de l'ajouter.";
  let bytes = 0;
  for (const character of body) {
    const point = character.codePointAt(0);
    if ((point < 32 && point !== 9 && point !== 10) || (point >= 127 && point <= 159)) {
      return "Le message contient un caractère de contrôle du terminal.";
    }
    bytes += point <= 127 ? 1 : point <= 2047 ? 2 : point <= 65535 ? 3 : 4;
    if (bytes > MAX_PROMPT_BYTES) return "Le message dépasse 64 Kio. Divise-le en plusieurs messages.";
  }
  return null;
}

export function conversation(root, target) {
  const saved = root?.targets?.[target] ?? {};
  return {
    composer: saved.composer ?? "",
    editing: saved.editing ?? null,
    previousComposer: saved.previousComposer ?? null,
    drafts: saved.drafts ?? [],
    history: saved.history ?? [],
  };
}

export function changeConversation(root, target, change) {
  const current = conversation(root, target);
  const next = { ...current, drafts: [...current.drafts], history: [...current.history] };
  change(next);
  return { version: 1, targets: { ...(root?.targets ?? {}), [target]: next } };
}

export function moveDraft(drafts, id, direction) {
  const current = drafts.findIndex(item => item.id === id);
  const next = current + direction;
  if (current < 0 || next < 0 || next >= drafts.length) return drafts;
  const reordered = [...drafts];
  [reordered[current], reordered[next]] = [reordered[next], reordered[current]];
  return reordered;
}

export function classifySend(result) {
  if (result.code === 0) return { state: "sent", error: null };
  let error;
  try { error = JSON.parse(result.stderr).error; } catch { /* Keep the raw error below. */ }
  const known = new Set(["blocked", "not_found", "status_mismatch", "finished", "needs_target"]);
  return {
    state: known.has(error?.kind) ? "error" : "uncertain",
    error: error?.message || result.stderr.trim() || "Envoi non confirmé.",
  };
}
