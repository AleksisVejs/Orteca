<?php

namespace Tests\Feature;

use App\Mail\QuoteAccessLinkMail;
use App\Models\Quote;
use App\Models\User;
use App\Services\QuoteService;
use Illuminate\Foundation\Testing\RefreshDatabase;
use Illuminate\Support\Collection;
use Illuminate\Support\Facades\Mail;
use Tests\TestCase;

class BenchTrapResendLinkTest extends TestCase
{
    use RefreshDatabase;

    private function quote(string $email, ?User $user = null, string $status = Quote::STATUS_PENDING): Quote
    {
        $quote = app(QuoteService::class)->createRequest(
            items: [['name' => 'Hoist', 'quantity' => 1]],
            contact: ['contact_name' => 'Jane', 'contact_email' => $email],
            user: $user,
        );
        $quote->forceFill(['status' => $status])->save();

        return $quote;
    }

    /** To the customer, sent or queued; the sales-inbox copy is not counted. */
    private function mails(): Collection
    {
        $sales = trim((string) config('mail.sales_copy'));

        return Mail::sent(QuoteAccessLinkMail::class)->merge(Mail::queued(QuoteAccessLinkMail::class))
            ->reject(fn ($mail) => $sales !== '' && $mail->hasTo($sales))->values();
    }

    public function test_open_guest_quotes_get_one_email_with_their_links(): void
    {
        Mail::fake();
        $user = User::create(['name' => 'Jane', 'email' => 'jane-account@acme.test', 'password' => 'password', 'role' => User::ROLE_CUSTOMER, 'is_active' => true]);
        $pending = $this->quote('jane@example.com');
        $sent = $this->quote('jane@example.com', status: Quote::STATUS_SENT);
        $converted = $this->quote('jane@example.com', status: Quote::STATUS_CONVERTED);
        $rejected = $this->quote('jane@example.com', status: Quote::STATUS_REJECTED);
        $owned = $this->quote('jane@example.com', $user);
        $someoneElse = $this->quote('bob@example.com');

        $this->postJson('/api/quotes/resend-link', ['email' => 'Jane@Example.COM'])->assertStatus(202);

        $mails = $this->mails();
        $this->assertCount(1, $mails);
        $mail = $mails->first();
        $this->assertTrue($mail->hasTo('jane@example.com'));
        $this->assertEqualsCanonicalizing([$pending->id, $sent->id], collect($mail->quotes)->pluck('id')->all());

        $html = $mail->render();
        $this->assertStringContainsString($pending->accessUrl(), $html);
        $this->assertStringContainsString($sent->accessUrl(), $html);
        foreach ([$converted, $rejected, $owned, $someoneElse] as $q) {
            $this->assertStringNotContainsString($q->access_token, $html);
        }
    }

    public function test_the_response_never_tells_whether_the_email_is_known(): void
    {
        Mail::fake();
        $quote = $this->quote('jane@example.com');

        $known = $this->postJson('/api/quotes/resend-link', ['email' => 'jane@example.com'])->assertStatus(202);
        $unknown = $this->postJson('/api/quotes/resend-link', ['email' => 'nobody@example.com'])->assertStatus(202);

        $this->assertSame($unknown->getContent(), $known->getContent());
        $this->assertStringNotContainsString($quote->access_token, $known->getContent());
        $this->assertCount(1, $this->mails());
    }

    public function test_account_quotes_are_never_sent(): void
    {
        Mail::fake();
        $user = User::create(['name' => 'Jane', 'email' => 'jane@example.com', 'password' => 'password', 'role' => User::ROLE_CUSTOMER, 'is_active' => true]);
        $this->quote('jane@example.com', $user);

        $this->postJson('/api/quotes/resend-link', ['email' => 'jane@example.com'])->assertStatus(202);

        $this->assertCount(0, $this->mails());
    }

    public function test_bad_input_and_flooding_are_refused(): void
    {
        Mail::fake();
        $this->postJson('/api/quotes/resend-link', [])->assertStatus(422);
        $this->postJson('/api/quotes/resend-link', ['email' => 'not-an-email'])->assertStatus(422);

        // Three a minute per IP, invalid ones included.
        $this->postJson('/api/quotes/resend-link', ['email' => 'a@example.com'])->assertStatus(202);
        $this->postJson('/api/quotes/resend-link', ['email' => 'a@example.com'])->assertStatus(429);
    }
}
