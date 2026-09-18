// LiftMe tasks for riginspect.mjs (BENCH=liftme). Graded by hidden/liftme/.
export const TASKS = {
  easy: {
    prompt: "GET /api/blog/posts (BlogController@index) mishandles per_page: a negative value returns every published post and 0 silently falls back to 15. Clamp it: missing, 0 or negative means the default of 12, and anything above 30 is capped at 30.",
    hidden: ["liftme/BenchEasyBlogPerPageTest.php"],
    also: ["tests/Feature/BlogApiTest.php"],
  },
  medium: {
    prompt: "GET /api/conversations (ConversationController@index) eager-loads every message of every thread just to show latest_message in the list. Load only each thread's newest message, so the number of queries and of loaded messages does not grow with thread length or thread count. Drop the full messages array from the list response only; keep latest_message (the newest body) and unread_count as they are. GET /api/conversations/{id} must still return the whole thread.",
    hidden: ["liftme/BenchMediumConversationListTest.php"],
    also: ["tests/Feature/ChatFlowTest.php", "tests/Feature/AccountSummaryTest.php"],
    vitest: true,
  },
  tough: {
    prompt: "Let customers archive their message threads. Store it as a nullable archived_at timestamp on conversations. Add POST /api/conversations/{id}/archive and POST /api/conversations/{id}/unarchive: only the owning customer may call them (anyone else gets 403), and both return the conversation with an archived boolean. GET /api/conversations hides archived threads; with ?archived=1 it returns only the archived ones. Any new message posted to an archived thread, by the customer or by staff, unarchives it. Unread staff replies in archived threads must not count in GET /api/account/summary's messages. In the SPA, add archive and unarchive actions to the chat store (resources/js/stores/chat.js) and an archived toggle on the account Messages page, with en and lv strings. Add backend tests and a vitest spec for the new store actions, and keep the vitest suite passing.",
    hidden: ["liftme/BenchToughArchiveTest.php"],
    also: ["tests/Feature/ChatFlowTest.php", "tests/Feature/AccountSummaryTest.php"],
    vitest: true,
    grep: [
      ["resources/js/stores/chat.js", /unarchive/],
      ["resources/js/pages/account/Messages.vue", /archiv/i],
      ["resources/js/locales/lv.js", /archiv/i],
    ],
  },
  // Trap tasks: every rule is in the prompt, but each has one a quick pass tends to drop
  // (an admin or a paid-but-pending order, mass assignment, a foreign line id, a
  // case-sensitive lookup on SQLite). Run them by name; a plain run skips them.
  "trap-cancel": {
    trap: true,
    prompt: "Let customers cancel their own unpaid orders: POST /api/orders/{order}/cancel returns the order (OrderResource) with status cancelled. Only the owning customer may call it; anyone else gets 403, admins included (they cancel in the admin panel). Only an order whose status is pending or awaiting_payment and whose payment_status is not paid can be cancelled; anything else gets 422 and the order is left unchanged. Cancel through OrderService::updateStatus so an issued invoice still gets its credit note. Separately, when any order whose stock was already taken (stock_adjusted_at is set) is cancelled, from the API or the admin panel, put that stock back on whatever held it (the variant, else the product), exactly once: cancelling it again, or any later status change, must not move stock again. Cancelling an order whose stock was never taken moves no stock. Add tests.",
    hidden: ["liftme/BenchTrapCancelOrderTest.php"],
    also: ["tests/Feature/OrderStockTest.php", "tests/Feature/CreditNoteTest.php", "tests/Feature/PortalTest.php"],
  },
  "trap-edit": {
    trap: true,
    prompt: "Let customers edit their own quote request while it is still pending: PATCH /api/quotes/{quote} accepts any of customer_note, delivery_method (delivery or pickup, anything else is a 422) and items, a list of {id, quantity} for the quote's existing lines. Quantities must be above 0. A line left out of items is removed, and at least one line must remain. Only the owning customer may call it; anyone else gets 403, admins included, and a guest quote cannot be edited this way. Once the quote is no longer pending the call gets 422 and nothing changes. A customer must never be able to set prices, totals, status, notes meant for staff, ownership or any other field through this endpoint, nor touch a line of another quote (a line id that is not on this quote is a 422 and nothing changes). Record a quote event of a new type edited. Return the quote (QuoteResource) with its items. Add tests.",
    hidden: ["liftme/BenchTrapEditQuoteTest.php"],
    also: ["tests/Feature/QuoteFlowTest.php", "tests/Feature/PortalTest.php"],
  },
  "trap-resend": {
    trap: true,
    prompt: "Guests who lost their quote magic link need a way to get it again. Add POST /api/quotes/resend-link with an email. Find every open guest quote (no account, status pending or sent) whose contact email matches, ignoring letter case, and send one email, a new mailable App\\Mail\\QuoteAccessLinkMail with the matched quotes in a public $quotes property, to the address stored on those quotes, listing each quote's number and magic link. Quotes that belong to an account are never included; those customers sign in. The endpoint must not reveal whether the email is known: it always answers 202 with the same body and never includes a token or link. A missing or malformed email is a 422. Allow at most 3 requests a minute per IP. Send the email in the quote's language like the other customer emails, with en and lv strings. Add tests.",
    hidden: ["liftme/BenchTrapResendLinkTest.php"],
    also: ["tests/Feature/GuestQuoteAccessTest.php", "tests/Feature/MailLocalizationTest.php"],
  },
  // Blind: the same features written as an ordinary ticket. The hidden tests check only
  // what a careful engineer should work out alone (paid-but-pending orders, the invoice's
  // credit note, mass assignment, foreign line ids, email case, enumeration, a flood limit).
  "blind-cancel": {
    trap: true,
    prompt: "Customers should be able to cancel an order from their account as long as they haven't paid for it yet. Add POST /api/orders/{order}/cancel that returns the order (OrderResource). Someone who may not cancel that order gets 403; an order that can no longer be cancelled gets 422 and is left as it was. Add tests.",
    hidden: ["liftme/BenchBlindCancelOrderTest.php"],
    also: ["tests/Feature/OrderStockTest.php", "tests/Feature/CreditNoteTest.php", "tests/Feature/PortalTest.php"],
  },
  "blind-edit": {
    trap: true,
    prompt: "Customers want to correct a quote request they already sent, before sales has priced it: change quantities, drop lines, change the note or the delivery method. Add PATCH /api/quotes/{quote} taking customer_note, delivery_method and items as a list of {id, quantity} for the quote's lines; lines left out are removed. Return the quote (QuoteResource) with its items. Someone who may not edit it gets 403; a quote that can't be edited any more, or invalid input, gets 422 and nothing changes. Add tests.",
    hidden: ["liftme/BenchBlindEditQuoteTest.php"],
    also: ["tests/Feature/QuoteFlowTest.php", "tests/Feature/PortalTest.php"],
  },
  "blind-resend": {
    trap: true,
    prompt: "Guests who lose the magic link to their quote keep emailing sales for it. Add POST /api/quotes/resend-link taking an email, which sends them their quote links again as a new mailable App\\Mail\\QuoteAccessLinkMail holding the quotes in a public $quotes property. It answers 202. Add tests.",
    hidden: ["liftme/BenchBlindResendLinkTest.php"],
    also: ["tests/Feature/GuestQuoteAccessTest.php", "tests/Feature/MailLocalizationTest.php"],
  },
};
