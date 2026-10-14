import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { appName, errorText, formatTime, groupByDay } from "./format";
import type { Category, Note } from "./types";

/** "all", "none" (uncategorized), or a category id. */
type Filter = "all" | "none" | number;

interface Props {
  focusNote: number | null;
  onFocused: () => void;
  onError: (message: string) => void;
}

/** Notes dictated with Right Alt (or saved from history), by category. */
export default function Notes({ focusNote, onFocused, onError }: Props) {
  const [notes, setNotes] = useState<Note[]>([]);
  const [categories, setCategories] = useState<Category[]>([]);
  const [filter, setFilter] = useState<Filter>("all");
  const [query, setQuery] = useState("");
  const [highlight, setHighlight] = useState<number | null>(null);

  const load = useCallback(() => {
    Promise.all([invoke<Note[]>("get_notes"), invoke<Category[]>("get_categories")])
      .then(([n, c]) => {
        setNotes(n);
        setCategories(c);
      })
      .catch((e) => onError(errorText(e)));
  }, [onError]);

  useEffect(() => {
    load();
    const stop = listen("notes-changed", load);
    return () => {
      stop.then((s) => s());
    };
  }, [load]);

  // If the selected category disappears (merged, emptied), fall back to all.
  useEffect(() => {
    if (typeof filter === "number" && !categories.some((c) => c.id === filter)) setFilter("all");
  }, [categories, filter]);

  // Jump to a note cited in Ask.
  useEffect(() => {
    if (focusNote === null || !notes.some((n) => n.id === focusNote)) return;
    setFilter("all");
    setQuery("");
    setHighlight(focusNote);
    requestAnimationFrame(() =>
      document.getElementById(`note-${focusNote}`)?.scrollIntoView({ block: "center", behavior: "smooth" }),
    );
    onFocused();
  }, [focusNote, notes, onFocused]);

  useEffect(() => {
    if (highlight === null) return;
    const t = setTimeout(() => setHighlight(null), 2500);
    return () => clearTimeout(t);
  }, [highlight]);

  const run = (command: string, args: Record<string, unknown>) =>
    invoke<string | null>(command, args)
      .then((warning) => {
        if (warning) onError(warning);
        load();
      })
      .catch((e) => {
        onError(errorText(e));
        load();
      });

  const uncategorized = notes.filter((n) => n.category_id === null).length;
  const groups = useMemo(() => {
    const q = query.trim().toLowerCase();
    const shown = notes.filter(
      (n) =>
        (filter === "all" || (filter === "none" ? n.category_id === null : n.category_id === filter)) &&
        (!q || n.text.toLowerCase().includes(q) || (n.category ?? "").toLowerCase().includes(q)),
    );
    return groupByDay(shown, (n) => n.created_at);
  }, [notes, filter, query]);

  return (
    <main className="view notes">
      <aside className="categories">
        <button className="category" aria-current={filter === "all"} onClick={() => setFilter("all")}>
          <span>All notes</span>
          <span className="count">{notes.length}</span>
        </button>
        {categories.map((c) => (
          <CategoryRow
            key={c.id}
            category={c}
            others={categories.filter((o) => o.id !== c.id)}
            selected={filter === c.id}
            onSelect={() => setFilter(c.id)}
            onRename={(name) => run("rename_category", { id: c.id, name })}
            onMerge={(into) => run("merge_categories", { from: c.id, into })}
          />
        ))}
        {uncategorized > 0 && (
          <button className="category" aria-current={filter === "none"} onClick={() => setFilter("none")}>
            <span>No category</span>
            <span className="count">{uncategorized}</span>
          </button>
        )}
      </aside>

      <section className="note-list">
        <input
          className="search"
          type="search"
          placeholder="Search notes"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        {notes.length === 0 ? (
          <p className="empty">No notes yet. Hold Right Alt, speak, and release.</p>
        ) : groups.length === 0 ? (
          <p className="empty">No matches.</p>
        ) : (
          groups.map(([day, rows]) => (
            <section key={day} className="day">
              <h2>{day}</h2>
              <ul>
                {rows.map((note) => (
                  <NoteItem
                    key={note.id}
                    note={note}
                    categories={categories}
                    highlighted={highlight === note.id}
                    onSave={(text) => run("update_note", { id: note.id, text })}
                    onCategory={(category) => run("set_note_category", { id: note.id, category })}
                    onDelete={() => run("delete_note", { id: note.id })}
                  />
                ))}
              </ul>
            </section>
          ))
        )}
        <footer className="footnote">Notes are kept until you delete them.</footer>
      </section>
    </main>
  );
}

function CategoryRow(props: {
  category: Category;
  others: Category[];
  selected: boolean;
  onSelect: () => void;
  onRename: (name: string) => void;
  onMerge: (into: number) => void;
}) {
  const { category, others, selected } = props;
  const [mode, setMode] = useState<"view" | "rename" | "merge">("view");
  const [name, setName] = useState(category.name);

  if (mode === "rename") {
    const done = () => {
      const trimmed = name.trim();
      if (trimmed && trimmed !== category.name) props.onRename(trimmed);
      setMode("view");
    };
    return (
      <form
        className="category-edit"
        onSubmit={(e) => {
          e.preventDefault();
          done();
        }}
      >
        <input autoFocus value={name} onChange={(e) => setName(e.target.value)} onBlur={done} aria-label="Category name" />
      </form>
    );
  }

  if (mode === "merge") {
    return (
      <div className="category-edit">
        <select
          autoFocus
          defaultValue=""
          aria-label={`Merge ${category.name} into`}
          onChange={(e) => {
            if (e.target.value) props.onMerge(Number(e.target.value));
            setMode("view");
          }}
          onBlur={() => setMode("view")}
        >
          <option value="" disabled>
            Merge {category.name} into…
          </option>
          {others.map((o) => (
            <option key={o.id} value={o.id}>
              {o.name}
            </option>
          ))}
        </select>
      </div>
    );
  }

  return (
    <div className="category-row">
      <button className="category" aria-current={selected} onClick={props.onSelect}>
        <span>{category.name}</span>
        <span className="count">{category.note_count}</span>
      </button>
      <span className="category-actions">
        <button
          onClick={() => {
            setName(category.name);
            setMode("rename");
          }}
        >
          Rename
        </button>
        {others.length > 0 && <button onClick={() => setMode("merge")}>Merge</button>}
      </span>
    </div>
  );
}

function NoteItem(props: {
  note: Note;
  categories: Category[];
  highlighted: boolean;
  onSave: (text: string) => void;
  onCategory: (category: string | null) => void;
  onDelete: () => void;
}) {
  const { note, categories } = props;
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(note.text);
  const [showRaw, setShowRaw] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [newCategory, setNewCategory] = useState<string | null>(null);
  const textarea = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    if (!confirmDelete) return;
    const t = setTimeout(() => setConfirmDelete(false), 4000);
    return () => clearTimeout(t);
  }, [confirmDelete]);

  useEffect(() => {
    const el = textarea.current;
    if (editing && el) {
      el.style.height = "auto";
      el.style.height = `${el.scrollHeight}px`;
    }
  }, [editing, draft]);

  const changed = note.raw_text.trim() !== note.text.trim();

  return (
    <li id={`note-${note.id}`} className="item" data-highlight={props.highlighted || undefined}>
      {editing ? (
        <form
          className="note-edit"
          onSubmit={(e) => {
            e.preventDefault();
            if (draft.trim() && draft.trim() !== note.text) props.onSave(draft.trim());
            setEditing(false);
          }}
        >
          <textarea ref={textarea} autoFocus value={draft} onChange={(e) => setDraft(e.target.value)} />
          <div className="note-edit-actions">
            <button type="submit">Save</button>
            <button type="button" onClick={() => setEditing(false)}>
              Cancel
            </button>
          </div>
        </form>
      ) : (
        <p className="item-text" data-raw={showRaw || undefined}>
          {showRaw ? note.raw_text : note.text}
        </p>
      )}

      <div className="item-meta">
        <span className="note-meta">
          {formatTime(note.created_at)}
          {" · "}
          {newCategory !== null ? (
            <form
              className="inline-form"
              onSubmit={(e) => {
                e.preventDefault();
                if (newCategory.trim()) props.onCategory(newCategory.trim());
                setNewCategory(null);
              }}
            >
              <input
                autoFocus
                placeholder="New category"
                value={newCategory}
                onChange={(e) => setNewCategory(e.target.value)}
                onBlur={() => setNewCategory(null)}
              />
            </form>
          ) : (
            <select
              className="category-select"
              aria-label="Category"
              value={note.category_id ?? ""}
              onChange={(e) => {
                const v = e.target.value;
                if (v === "new") setNewCategory("");
                else props.onCategory(v === "" ? null : (categories.find((c) => String(c.id) === v)?.name ?? null));
              }}
            >
              {categories.map((c) => (
                <option key={c.id} value={c.id}>
                  {c.name}
                </option>
              ))}
              <option value="">No category</option>
              <option value="new">New category…</option>
            </select>
          )}
          {note.source_app && ` · ${appName(note.source_app)}`}
        </span>
        {!editing && (
          <span className="item-actions" data-pinned={confirmDelete || undefined}>
            <button
              onClick={() => {
                setDraft(note.text);
                setShowRaw(false);
                setEditing(true);
              }}
            >
              Edit
            </button>
            {changed && <button onClick={() => setShowRaw(!showRaw)}>{showRaw ? "Cleaned" : "Original"}</button>}
            {confirmDelete ? (
              <button className="danger" onClick={props.onDelete}>
                Delete for good?
              </button>
            ) : (
              <button onClick={() => setConfirmDelete(true)}>Delete</button>
            )}
          </span>
        )}
      </div>
    </li>
  );
}
