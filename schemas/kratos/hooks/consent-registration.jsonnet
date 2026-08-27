// Body of the after-registration consent web hook: what Kratos posts to
// dovecote's POST /internal/consent (capsules::ConsentHookPayload).
//
// Only the state and where it came from. The notice version and the
// timestamp are stamped server-side, because Kratos has no idea which
// privacy notice was on screen and a caller-supplied version would be an
// assertion rather than a record.
//
// The trait is read defensively: an identity registered before the trait
// existed, or through a flow that never rendered the checkbox, has no
// marketing_consent object at all, and reading straight through would
// fail the hook rather than report the truth, which is that nobody
// consented. dovecote writes no row for that case.
function(ctx) {
  identity_id: ctx.identity.id,
  granted:
    if std.objectHas(ctx.identity.traits, 'marketing_emails')
    then ctx.identity.traits.marketing_emails
    else false,
  source: 'registration',
  // Kept out of the payload entirely when the hook context carries no
  // flow, rather than sent as null: it is a cross-reference into Kratos's
  // own tables, and the field is optional on the receiving side.
  [if std.objectHas(ctx, 'flow') && std.objectHas(ctx.flow, 'id') then 'flow_id']:
    ctx.flow.id,

  // The address and browser the change came from are NOT sent, and the
  // columns waiting for them stay empty. The privacy notice discloses
  // addresses and user agents only as transient web logs kept for
  // debugging and abuse prevention; keeping one against an identity as
  // consent evidence is a different purpose with a different retention,
  // so the notice needs a line about it first. When it has one, add:
  //   ip: ctx.request_headers['X-Forwarded-For'][0],
  //   user_agent: ctx.request_headers['User-Agent'][0],
  // and nothing else changes -- dovecote already stores both.
}
