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
};
