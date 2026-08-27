// Body of the after-settings consent web hook: what Kratos posts to
// dovecote's POST /internal/consent (capsules::ConsentHookPayload).
//
// Only the state and where it came from. The notice version and the
// timestamp are stamped server-side, because Kratos has no idea which
// privacy notice was on screen and a caller-supplied version would be an
// assertion rather than a record.
//
// The trait is read defensively, and here that is the common case rather
// than the edge one: every identity that existed before the trait did
// carries no marketing_consent object, and each of them will save
// settings at some point. Reading straight through would fail the hook
// instead of reporting the truth, which is that nobody consented.
// dovecote writes no row when the state has not moved, so those saves
// leave the table alone.
function(ctx) {
  identity_id: ctx.identity.id,
  granted:
    if std.objectHas(ctx.identity.traits, 'marketing_emails')
    then ctx.identity.traits.marketing_emails
    else false,
  source: 'settings',
  // Kept out of the payload entirely when the hook context carries no
  // flow, rather than sent as null: it is a cross-reference into Kratos's
  // own tables, and the field is optional on the receiving side.
  [if std.objectHas(ctx, 'flow') && std.objectHas(ctx.flow, 'id') then 'flow_id']:
    ctx.flow.id,
}
