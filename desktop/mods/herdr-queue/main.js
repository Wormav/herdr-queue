import { View, div, svg } from "gpui-kit";
import { h_flex, v_flex, TextareaState, Textarea } from "gpui-base";
import { agents, exec, nextCommand, notify, panes, props, run, setShared, shared, theme } from "herdr";
import { changeConversation, classifySend, conversation, moveDraft, promptError } from "./queue-state.js";

const STORE = "herdr-queue.desktop";

export default class HerdrQueue extends View {
  init(_props, cx) {
    this.target = props()?.terminal;
    this.tab = props()?.tab;
    this.history = false;
    this.busyId = null;
    this.notice = "";
    const saved = this.current();
    this.input = TextareaState.new({ placeholder: "Écris le prochain message…", value: saved.composer, rows: 5 });
    this.input.on("change", (_event, cx) => {
      this.write(state => { state.composer = this.input.value(); });
      cx.notify();
    });
    this.input.on("submit", (event, cx) => {
      if (event.shift) return;
      this.save(cx);
    });
    this.input.focus();
    if (saved.drafts.some(item => item.state === "sending")) {
      this.write(state => {
        state.drafts = state.drafts.map(item => item.state === "sending"
          ? { ...item, state: "uncertain", error: "L'envoi a été interrompu. Vérifie le terminal avant de réessayer." }
          : item);
      });
    }
    cx.spawn(async (cx) => {
      for (;;) {
        const command = await nextCommand();
        if (command === "send_first") this.sendFirst(cx);
        if (command === "focus") this.input.focus();
        cx.notify();
      }
    });
  }

  current() {
    return conversation(shared(STORE), this.target);
  }

  write(change) {
    setShared(STORE, changeConversation(shared(STORE), this.target, change));
  }

  save(cx) {
    const body = this.input.value();
    const error = promptError(body);
    if (error) { this.notice = error; cx.notify(); return; }
    const editing = this.current().editing;
    const restore = this.current().previousComposer ?? "";
    this.write(state => {
      if (editing) {
        state.drafts = state.drafts.map(item => item.id === editing
          ? { ...item, body, state: "draft", error: null }
          : item);
        state.composer = restore;
        state.editing = null;
        state.previousComposer = null;
      } else {
        state.drafts.push({ id: `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`, body, state: "draft", error: null });
        state.composer = "";
      }
    });
    this.input.set_value(editing ? restore : "");
    this.notice = "";
    cx.notify();
  }

  edit(item, cx) {
    if (this.busyId || !["draft", "error"].includes(item.state)) return;
    this.write(state => {
      if (!state.editing) state.previousComposer = state.composer;
      state.editing = item.id;
      state.composer = item.body;
    });
    this.input.set_value(item.body);
    this.input.focus();
    this.history = false;
    cx.notify();
  }

  cancelEdit(cx) {
    const restore = this.current().previousComposer ?? "";
    this.write(state => { state.composer = restore; state.editing = null; state.previousComposer = null; });
    this.input.set_value(restore);
    cx.notify();
  }

  remove(item, cx) {
    if (this.busyId || !["draft", "error"].includes(item.state)) return;
    if (this.current().editing === item.id) this.cancelEdit(cx);
    this.write(state => { state.drafts = state.drafts.filter(draft => draft.id !== item.id); });
    cx.notify();
  }

  move(item, direction, cx) {
    if (this.busyId || !["draft", "error"].includes(item.state)) return;
    this.write(state => { state.drafts = moveDraft(state.drafts, item.id, direction); });
    cx.notify();
  }

  sendFirst(cx) {
    const first = this.current().drafts.find(item => ["draft", "error"].includes(item.state));
    if (first) this.send(first, cx);
  }

  send(item, cx) {
    if (this.busyId || !["draft", "error"].includes(item.state)) return;
    this.busyId = item.id;
    this.write(state => {
      state.drafts = state.drafts.map(draft => draft.id === item.id ? { ...draft, state: "sending", error: null } : draft);
    });
    cx.spawn(async (cx) => {
      let outcome;
      const agent = agents().find(candidate => candidate.address === this.target);
      if (!agent || agent.status === "blocked") {
        outcome = { state: "error", error: agent ? "L'agent attend une réponse interactive." : "L'agent n'est plus disponible." };
      } else {
        try {
          const result = await exec("hc", ["send", "--json", this.target, item.body], { timeout: 90000 });
          outcome = classifySend(result);
        } catch (error) {
          outcome = { state: "uncertain", error: `Envoi non confirmé : ${String(error)}` };
        }
      }
      this.write(state => {
        const index = state.drafts.findIndex(draft => draft.id === item.id && draft.state === "sending");
        if (index < 0) return;
        if (outcome.state === "sent") {
          const [sent] = state.drafts.splice(index, 1);
          state.history.unshift({ ...sent, state: "sent", error: null, sentAt: new Date().toISOString() });
        } else {
          state.drafts[index] = { ...state.drafts[index], ...outcome };
        }
      });
      this.busyId = null;
      if (outcome.state === "sent") notify({ title: "Queue", body: "Message transmis au terminal." });
      cx.notify();
    });
  }

  resolve(item, sent, cx) {
    if (item.state !== "uncertain") return;
    this.write(state => {
      const index = state.drafts.findIndex(draft => draft.id === item.id);
      if (index < 0) return;
      const [draft] = state.drafts.splice(index, 1);
      if (sent) state.history.unshift({ ...draft, state: "sent", error: null, sentAt: new Date().toISOString() });
      else state.drafts.splice(index, 0, { ...draft, state: "draft", error: null });
    });
    cx.notify();
  }

  reuse(item, cx) {
    this.history = false;
    this.write(state => { state.composer = item.body; state.editing = null; state.previousComposer = null; });
    this.input.set_value(item.body);
    this.input.focus();
    cx.notify();
  }

  closePane() {
    const pane = panes().find(candidate => candidate.mod === "herdr-queue" && candidate.tab === this.tab);
    if (pane) run("pane.close", { pane: pane.pane });
  }

  button(id, label, action, primary = false) {
    const t = theme();
    return h_flex().id(id).h(27).px(9).items_center().rounded(6).flex_none().cursor_pointer()
      .bg(primary ? t.solid : t.raised).text_color(primary ? t.solid_foreground : t.foreground)
      .text_size(11.5).hover(style => style.bg(primary ? t.accent : t.hover))
      .on_click(action).child(label);
  }

  card(item) {
    const t = theme();
    const uncertain = item.state === "uncertain";
    const pending = item.state === "sending";
    const preview = item.body.replace(/\s*\n\s*/g, " ↵ ").slice(0, 500);
    return v_flex().id(`draft-${item.id}`).p(10).gap(8).rounded(8).border(1).border_color(uncertain ? t.blocked : t.border).bg(t.raised)
      .child(div().text_size(12.3).text_color(t.foreground).child(preview + (item.body.length > 500 ? "…" : "")))
      .when(Boolean(item.error), element => element.child(div().text_size(11).text_color(uncertain ? t.blocked : t.dim).child(item.error)))
      .when(uncertain, element => element.child(h_flex().gap(5)
        .child(this.button(`sent-${item.id}`, "Déjà transmis", (_event, cx) => this.resolve(item, true, cx)))
        .child(this.button(`retry-${item.id}`, "Remettre en brouillon", (_event, cx) => this.resolve(item, false, cx)))))
      .when(!uncertain, element => element.child(h_flex().gap(5).flex_wrap()
        .child(this.button(`send-${item.id}`, pending ? "Envoi…" : "Envoyer", (_event, cx) => this.send(item, cx), !pending))
        .child(this.button(`edit-${item.id}`, "Modifier", (_event, cx) => this.edit(item, cx)))
        .child(this.button(`up-${item.id}`, "↑", (_event, cx) => this.move(item, -1, cx)))
        .child(this.button(`down-${item.id}`, "↓", (_event, cx) => this.move(item, 1, cx)))
        .child(this.button(`remove-${item.id}`, "Supprimer", (_event, cx) => this.remove(item, cx)))));
  }

  historyCard(item) {
    const t = theme();
    return v_flex().id(`history-${item.id}`).p(10).gap(7).rounded(8).border(1).border_color(t.border).bg(t.raised)
      .child(div().text_size(10.5).text_color(t.dim).child(item.sentAt?.slice(0, 16).replace("T", " ") ?? "Transmis"))
      .child(div().text_size(12).text_color(t.foreground).child(item.body.replace(/\s*\n\s*/g, " ↵ ").slice(0, 500)))
      .child(h_flex().child(this.button(`reuse-${item.id}`, "Réutiliser", (_event, cx) => this.reuse(item, cx))));
  }

  render() {
    const t = theme();
    const state = this.current();
    const agent = agents().find(candidate => candidate.address === this.target);
    const count = state.drafts.filter(item => item.state !== "sent").length;
    return v_flex().id("queue-root").size_full().min_h(0).bg(t.surface).text_color(t.foreground)
      .child(h_flex().h(36).px(11).gap(8).items_center().border_b(1).border_color(t.border)
        .child(svg("mods/herdr-queue/queue.svg").size(16).text_color(t.accent))
        .child(div().flex_1().text_size(13).font_semibold().child("Queue"))
        .child(div().text_size(11).text_color(t.dim).child(agent?.program?.name ?? this.target ?? "Agent absent"))
        .child(this.button("close-queue", "×", () => this.closePane())))
      .child(h_flex().px(11).py(9).gap(6).border_b(1).border_color(t.border)
        .child(this.button("queue-tab", `À envoyer (${count})`, (_event, cx) => { this.history = false; cx.notify(); }, !this.history))
        .child(this.button("history-tab", `Historique (${state.history.length})`, (_event, cx) => { this.history = true; cx.notify(); }, this.history)))
      .child(v_flex().id("queue-list").flex_1().min_h(0).p(10).gap(8).overflow_y_scrollbar()
        .when(this.history && !state.history.length, element => element.child(div().p(12).text_color(t.dim).child("Aucun message transmis.")))
        .when(!this.history && !count, element => element.child(div().p(12).text_color(t.dim).child("Prépare un message pendant que l'agent travaille.")))
        .children(this.history ? state.history.map(item => this.historyCard(item)) : state.drafts.map(item => this.card(item))))
      .when(!this.history, element => element.child(v_flex().p(11).gap(8).border_t(1).border_color(t.border)
        .child(Textarea.new(this.input).h(98).px(9).py(7).rounded(8).border(1).border_color(t.input).bg(t.background).text_size(12).text_color(t.foreground))
        .when(Boolean(this.notice), area => area.child(div().text_size(11).text_color(t.blocked).child(this.notice)))
        .child(h_flex().justify_between().items_center()
          .child(div().text_size(10.5).text_color(t.dim).child("Envoi manuel · 64 Kio max"))
          .child(h_flex().gap(6)
            .when(Boolean(state.editing), row => row.child(this.button("cancel-edit", "Annuler", (_event, cx) => this.cancelEdit(cx))))
            .child(this.button("add-draft", state.editing ? "Enregistrer" : "Ajouter à la liste", (_event, cx) => this.save(cx), true))))));
  }
}
