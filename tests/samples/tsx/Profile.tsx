// A profile page: the property suite's TSX sample.
import React, { useCallback, useEffect, useMemo, useState } from "react";
import type { ReactNode } from "react";

export interface User {
  id: string;
  name: string;
  email: string;
  roles: string[];
  avatarUrl?: string;
}

type Props = {
  userId: string;
  fetchUser: (id: string) => Promise<User>;
  onSave?: (user: User) => void;
  children?: ReactNode;
};

const ROLE_LABELS: Record<string, string> = {
  admin: "Administrator",
  editor: "Editor",
  viewer: "Viewer",
};

export function Avatar({ url, name, size = 48 }: { url?: string; name: string; size?: number }) {
  if (!url) {
    return <span className="avatar avatar--initials">{name.slice(0, 1)}</span>;
  }
  return <img className="avatar" src={url} alt={name} width={size} height={size} />;
}

function RoleList({ roles }: { roles: string[] }) {
  return (
    <ul className="roles">
      {roles.map((role) => (
        <li key={role} title={role}>
          {ROLE_LABELS[role] ?? role}
        </li>
      ))}
    </ul>
  );
}

export class ErrorBoundary extends React.Component<{ children: ReactNode }, { failed: boolean }> {
  state = { failed: false };

  static getDerivedStateFromError(): { failed: boolean } {
    return { failed: true };
  }

  render() {
    return this.state.failed ? <p role="alert">Something went wrong.</p> : this.props.children;
  }
}

export default function Profile({ userId, fetchUser, onSave, children }: Props) {
  const [user, setUser] = useState<User | null>(null);
  const [draft, setDraft] = useState("");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    fetchUser(userId)
      .then((loaded) => {
        if (!cancelled) {
          setUser(loaded);
          setDraft(loaded.name);
        }
      })
      .catch((e: Error) => setError(e.message));
    return () => {
      cancelled = true;
    };
  }, [userId, fetchUser]);

  const changed = useMemo(() => user !== null && draft.trim() !== user.name, [user, draft]);

  const save = useCallback(() => {
    if (user === null || !changed) {
      return;
    }
    const updated = { ...user, name: draft.trim() };
    setUser(updated);
    onSave?.(updated);
  }, [user, draft, changed, onSave]);

  if (error !== null) {
    return <p className="error">Could not load {userId}: {error}</p>;
  }
  if (user === null) {
    return <p aria-busy="true">Loading…</p>;
  }

  return (
    <ErrorBoundary>
      <section className="profile">
        <header>
          <Avatar url={user.avatarUrl} name={user.name} />
          <h1>{user.name}</h1>
          <a href={`mailto:${user.email}`}>{user.email}</a>
        </header>
        <RoleList roles={user.roles} />
        <label>
          Name
          <input value={draft} onChange={(e) => setDraft(e.target.value)} disabled={!onSave} />
        </label>
        <button type="button" onClick={save} disabled={!changed}>
          Save
        </button>
        {children}
      </section>
    </ErrorBoundary>
  );
}
