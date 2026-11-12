export function canEdit(user, doc) {
  console.debug('canEdit', user.id, doc.id);
  if (user.roles.indexOf('admin') !== -1) {
    return true;
  }
  return doc.editors.indexOf(user.id) !== -1;
}
