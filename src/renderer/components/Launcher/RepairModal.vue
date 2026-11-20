<template>
  <ConfirmModal
    :title="$t('#L_C_REPAIR_TITLE')"
    :text="$t('#L_GM_REPAIR_PICK')"
    :note="warnConfigs ? $t('#L_GM_REPAIR_CFG_WARN') : null"
    :confirmLabel="$t('#L_GM_REPAIR_FETCH', String(chosenFiles.length), formatSize(chosenBytes))"
    :confirmDisabled="!chosenFiles.length"
    @resolve="onResolve"
  >
    <div class="rp">
      <div class="rp__head">
        <button type="button" class="rp__btn" @click="setAll(true)">{{ $t("#L_DEMOS_SELECT_ALL") }}</button>
        <button type="button" class="rp__btn" @click="setAll(false)">{{ $t("#L_DEMOS_CLEAR_ALL") }}</button>
      </div>

      <div class="rp__scroll">
        <section v-for="section in sections" :key="section.id" class="rp__section">
          <span v-if="section.title" class="rp__title">{{ section.title }}</span>
          <ul class="rp__list">
            <li v-for="group in section.groups" :key="group.key">
              <div v-if="group.folder" class="rp__row">
                <button type="button" class="rp__open" :aria-expanded="open[group.key] ? 'true' : 'false'"
                  :aria-label="$t('#L_GM_REPAIR_FOLDER', group.key || '/')" @click="toggleOpen(group.key)">
                  <i :class="open[group.key] ? 'fas fa-chevron-down' : 'fas fa-chevron-right'" aria-hidden="true"></i>
                </button>
                <label class="rp__pick">
                  <input type="checkbox" :checked="groupState(group) === 'all'" :indeterminate="groupState(group) === 'some'"
                    @change="setGroup(group, $event.target.checked)" />
                  <span class="rp__name">{{ (group.key || "") + "/" }} ({{ group.files.length }})</span>
                </label>
                <span class="rp__size">{{ formatSize(sizeOf(group.files)) }}</span>
              </div>
              <ul v-if="!group.folder || open[group.key]" class="rp__files" :class="{ nested: group.folder }">
                <li v-for="file in group.files" :key="file.path" class="rp__row">
                  <label class="rp__pick">
                    <input type="checkbox" :checked="!!chosen[file.path]" @change="toggle(file.path)" />
                    <span class="rp__name">{{ group.folder ? file.path.slice(group.key.length).replace(/^\//, "") : file.path }}</span>
                    <span class="rp__tag">{{ tag(file) }}</span>
                  </label>
                  <span class="rp__size">{{ formatSize(file.size) }}</span>
                </li>
              </ul>
            </li>
          </ul>
        </section>
      </div>
    </div>
  </ConfirmModal>
</template>

<script>
import ConfirmModal from "./ConfirmModal";
import { formatSize } from "./file-format";

const TAGS = {
  missing: "#L_GM_REPAIR_MISSING",
  damaged: "#L_GM_REPAIR_DAMAGED",
  "cfg-missing": "#L_GM_REPAIR_MISSING",
  "cfg-changed": "#L_GM_REPAIR_CHANGED",
};

// Окно ремонта: файлы, найденные проверкой, с галочками. Отвечает списком
// отмеченных путей или false. Изменённые игроком cfg по умолчанию не отмечены.
export default {
  name: "RepairModal",
  components: { ConfirmModal },
  emits: ["resolve"],
  props: {
    // [{ path, size, kind: "missing" | "damaged" | "cfg-missing" | "cfg-changed" }]
    files: { type: Array, required: true },
  },
  data() {
    const chosen = {};
    for (const file of this.files) chosen[file.path] = file.kind !== "cfg-changed";
    return { chosen, open: {} };
  },
  computed: {
    sections() {
      const cfg = (file) => file.kind === "cfg-missing" || file.kind === "cfg-changed";
      return [
        { id: "build", title: null, groups: this.groupsOf(this.files.filter((file) => !cfg(file))) },
        { id: "cfg", title: this.$t("#L_GM_REPAIR_CFG"), groups: this.groupsOf(this.files.filter(cfg)) },
      ].filter((section) => section.groups.length);
    },
    chosenFiles() {
      return this.files.filter((file) => this.chosen[file.path]);
    },
    chosenBytes() {
      return this.sizeOf(this.chosenFiles);
    },
    warnConfigs() {
      return this.chosenFiles.some((file) => file.kind === "cfg-changed");
    },
  },
  methods: {
    formatSize(bytes) {
      return formatSize(bytes, this.$t);
    },
    tag(file) {
      return this.$t(TAGS[file.kind]);
    },
    sizeOf(files) {
      return files.reduce((sum, file) => sum + (file.size || 0), 0);
    },
    // Больше 30 файлов в разделе — список сворачивается по папкам.
    groupsOf(files) {
      if (files.length <= 30) return files.length ? [{ key: "", folder: false, files }] : [];

      const byDir = new Map();
      for (const file of files) {
        const dir = file.path.includes("/") ? file.path.slice(0, file.path.lastIndexOf("/")) : "";
        if (!byDir.has(dir)) byDir.set(dir, []);
        byDir.get(dir).push(file);
      }
      return [...byDir.keys()].sort().map((key) => ({ key, folder: true, files: byDir.get(key) }));
    },
    toggle(path) {
      this.chosen = { ...this.chosen, [path]: !this.chosen[path] };
    },
    setAll(value) {
      const next = {};
      for (const file of this.files) next[file.path] = value;
      this.chosen = next;
    },
    groupState(group) {
      const count = group.files.filter((file) => this.chosen[file.path]).length;
      return count === group.files.length ? "all" : count ? "some" : "none";
    },
    setGroup(group, value) {
      const next = { ...this.chosen };
      for (const file of group.files) next[file.path] = value;
      this.chosen = next;
    },
    toggleOpen(key) {
      this.open = { ...this.open, [key]: !this.open[key] };
    },
    onResolve(accepted) {
      this.$emit("resolve", accepted ? this.chosenFiles.map((file) => file.path) : false);
    },
  },
};
</script>

<style lang="scss" scoped>
.rp {
  display: flex;
  flex-direction: column;
  gap: 8px;
  margin-top: 14px;
  min-width: 0;

  &__head { display: flex; gap: 8px; }

  &__btn {
    padding: 5px 12px;
    border: 1px solid var(--l-border-strong);
    border-radius: var(--l-radius-sm);
    background: rgba(255, 255, 255, 0.05);
    color: var(--l-text);
    font-family: inherit;
    font-size: 12.5px;
    font-weight: 600;
    cursor: pointer;
    transition: background var(--l-fast);
    &:hover { background: rgba(255, 255, 255, 0.1); }
  }

  &__scroll {
    max-height: 260px;
    overflow-y: auto;
    padding: 8px 10px;
    background: rgba(255, 255, 255, 0.05);
    border: 1px solid var(--l-border);
    border-radius: var(--l-radius-sm);
  }

  &__section + &__section { margin-top: 10px; }

  &__title {
    display: block;
    margin: 2px 0 4px;
    color: var(--l-text-dim);
    font-size: 11.5px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.08em;
  }

  &__list, &__files { list-style: none; margin: 0; padding: 0; }
  &__files.nested { padding-left: 24px; }

  &__row {
    display: flex;
    align-items: center;
    gap: 6px;
    min-height: 26px;
    color: var(--l-text);
    font-size: 12.5px;
  }

  &__open {
    display: grid;
    place-items: center;
    width: 18px;
    height: 18px;
    flex: none;
    padding: 0;
    border: 0;
    background: transparent;
    color: var(--l-text-dim);
    font-size: 10px;
    cursor: pointer;
    &:hover { color: var(--l-text); }
  }

  &__pick {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: 1;
    min-width: 0;
    cursor: pointer;
    input { accent-color: var(--l-accent); margin: 0; flex: none; }
  }

  &__name { min-width: 0; word-break: break-all; }
  &__tag { flex: none; color: var(--l-text-faint); font-size: 11.5px; }
  &__size { flex: none; color: var(--l-text-faint); font-size: 12px; font-variant-numeric: tabular-nums; }

  &__btn:focus-visible, &__open:focus-visible { outline: 2px solid var(--l-accent); outline-offset: 2px; }
}
</style>
