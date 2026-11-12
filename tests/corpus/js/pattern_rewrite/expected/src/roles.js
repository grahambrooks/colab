export function canEdit(user, doc) {
  if (user.roles.includes('admin')) {
    return true;
  }
  return doc.editors.includes(user.id);
}
