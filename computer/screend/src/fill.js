(origin, fields, pick) => {
  // The verified write of a Vault fill (ADR-0013). screend calls this
  // function in an isolated world of the top frame of the daemon's tab,
  // so a script of the page cannot change what it reads. It gets the
  // origin that the daemon verified and the values to write. It answers
  // null when it wrote every value, or the reason why it stopped.
  //
  // When an input has the focus, the first value goes into it, and each
  // next value goes into the first field of its kind after the previous
  // field in the same form. A focused input of the wrong kind, or one
  // outside the login form, stops the fill: the function does not look
  // around a focus that the page gave.
  //
  // While no input has the focus, it answers { wait: reason }, because
  // the page can give the focus to its field after the load event, and
  // screend asks again. When the wait ends, screend asks with `pick`, and
  // the function finds the login fields itself, as a password manager
  // does: the first password field on the screen, and the last text or
  // email field before it in the same form (among the fields of no form
  // when the password field has no form). A code alone goes into the one
  // one-time-code field on the screen, or else into the one text field on
  // the screen.
  //
  // The function finds every field before it writes one, so a page that
  // does not have them gets no text at all.
  const names = { text: "text field", password: "password field" };
  const oneTimeCode = (input) =>
    (input.getAttribute("autocomplete") || "")
      .toLowerCase()
      .split(/\s+/)
      .includes("one-time-code");
  const kinds = {
    text: (input) =>
      input.type === "text" ||
      input.type === "email" ||
      (input.type !== "password" && oneTimeCode(input)),
    password: (input) => input.type === "password",
  };
  // A field that a person can see and that is not disabled.
  const usable = (input) => !input.disabled && input.getClientRects().length > 0;
  const inputs = (elements) =>
    Array.from(elements).filter((element) => element instanceof HTMLInputElement);
  const everyInput = () => inputs(document.querySelectorAll("input"));
  // The fields of the form of `field`, or the fields of no form.
  const scope = (field) =>
    field.form ? inputs(field.form.elements) : everyInput().filter((input) => !input.form);
  const following = (previous, kind) => {
    const list = scope(previous);
    return list
      .slice(list.indexOf(previous) + 1)
      .find((input) => kinds[kind](input) && usable(input));
  };
  // The username field of a password field: the last text or email
  // field before it in the same form.
  const usernameOf = (password) => {
    const list = scope(password);
    return list
      .slice(0, list.indexOf(password))
      .filter((input) => (input.type === "text" || input.type === "email") && usable(input))
      .pop();
  };
  // The fields for the values when no input has the focus, or the reason
  // why the page has none.
  const picked = () => {
    const shape = fields.map(({ kind }) => kind).join(",");
    if (shape === "text") {
      const codes = everyInput().filter(
        (input) => kinds.text(input) && oneTimeCode(input) && usable(input),
      );
      if (codes.length === 1) {
        return [{ field: codes[0], kind: "text" }];
      }
      if (codes.length > 1) {
        return `no field has the focus, and the page shows ${codes.length} one-time-code fields`;
      }
      const texts = everyInput().filter((input) => kinds.text(input) && usable(input));
      if (texts.length === 1) {
        return [{ field: texts[0], kind: "text" }];
      }
      return `no field has the focus, and the page shows ${texts.length} text fields and no one-time-code field`;
    }
    const password = everyInput().find((input) => kinds.password(input) && usable(input));
    if (!password) {
      return "no field has the focus, and the page shows no password field";
    }
    if (shape === "password") {
      return [{ field: password, kind: "password" }];
    }
    if (shape === "text,password") {
      const username = usernameOf(password);
      if (!username) {
        return "the login form has no text field before the password field";
      }
      return [
        { field: username, kind: "text" },
        { field: password, kind: "password" },
      ];
    }
    return "the fill has no rule to find fields for these values";
  };
  // The reason one field cannot take a value of its kind, or null.
  const unfit = (field, kind) => {
    if (!kinds[kind](field)) {
      return `the field that has the focus is not a ${names[kind]}`;
    }
    if (field.disabled || field.readOnly) {
      return `the ${names[kind]} does not take text`;
    }
    if (field.getClientRects().length === 0) {
      return `the ${names[kind]} is not on the screen`;
    }
    return null;
  };

  if (location.origin !== origin) {
    return `the page is on ${location.origin}, not on ${origin}`;
  }
  const focused = document.activeElement;
  let targets = [];
  if (focused instanceof HTMLInputElement) {
    for (const { kind } of fields) {
      const previous = targets[targets.length - 1];
      const field = previous ? following(previous.field, kind) : focused;
      if (!field) {
        return `the page has no ${names[kind]} after the ${names[previous.kind]}`;
      }
      const reason = unfit(field, kind);
      if (reason) {
        return reason;
      }
      targets.push({ field, kind });
    }
  } else if (pick) {
    const found = picked();
    if (typeof found === "string") {
      return found;
    }
    for (const { field, kind } of found) {
      const reason = unfit(field, kind);
      if (reason) {
        return reason;
      }
    }
    targets = found;
  } else {
    return { wait: `no ${names[fields[0].kind]} has the focus on the page` };
  }
  // A username goes only into the username field of the password field
  // that follows it.
  for (const [index, { field, kind }] of targets.entries()) {
    const previous = targets[index - 1];
    if (kind === "password" && previous?.kind === "text" && usernameOf(field) !== previous.field) {
      return "the text field that has the focus is not the username field of the login form";
    }
  }

  // Before each write, the function gives the field the focus when it
  // does not have it, and checks again that the top-level origin is the
  // verified one, that the field has the focus, and that the field takes
  // text of its kind. No script of the page runs between that check and
  // the write.
  for (const [index, { field, kind }] of targets.entries()) {
    if (document.activeElement !== field) {
      // The focus handlers of the page run here, before the check.
      field.focus();
    }
    if (location.origin !== origin) {
      return `the page is now on ${location.origin}, not on ${origin}`;
    }
    if (document.activeElement !== field) {
      return `the ${names[kind]} did not keep the focus`;
    }
    const reason = unfit(field, kind);
    if (reason) {
      return reason;
    }
    field.value = fields[index].text;
    field.dispatchEvent(new Event("input", { bubbles: true, composed: true }));
    field.dispatchEvent(new Event("change", { bubbles: true }));
  }
  return null;
}
