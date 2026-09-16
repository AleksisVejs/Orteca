<?php

namespace Tests\Feature;

use App\Models\Admin;
use App\Models\Client;
use App\Models\Equipment;
use App\Models\User;
use Illuminate\Foundation\Testing\RefreshDatabase;
use Illuminate\Support\Facades\DB;
use Illuminate\Support\Facades\Hash;
use Tests\TestCase;

class BenchMediumSearchQueriesTest extends TestCase
{
    use RefreshDatabase;

    private Admin $admin;

    private User $adminUser;

    protected function setUp(): void
    {
        parent::setUp();
        $this->adminUser = $this->makeUser('admin');
        $this->admin = Admin::create([
            'user_id' => $this->adminUser->id,
            'company_name' => 'Search Co',
            'company_registration_number' => '40000000003',
            'verification_status' => 'verified',
        ]);
        $this->entitle($this->admin);
    }

    private function makeUser(string $type): User
    {
        return User::factory()->create([
            'type' => $type,
            'password' => Hash::make('Password1!'),
            'email_verified_at' => now(),
            'is_active' => true,
        ]);
    }

    private function makeHits(string $word, int $count): void
    {
        for ($i = 0; $i < $count; $i++) {
            $user = $this->makeUser('client');
            Client::create(['user_id' => $user->id, 'admin_id' => $this->admin->id, 'company_name' => "Holder $word $i"]);
            $equipment = Equipment::create(['name' => "Zq$word unit $i", 'admin_id' => $this->admin->id]);
            DB::table('client_equipment')->insert(['equipment_id' => $equipment->id, 'client_id' => $user->id]);
        }
    }

    private function search(string $query): array
    {
        DB::flushQueryLog();
        DB::enableQueryLog();
        $response = $this->actingAs($this->adminUser)->getJson('/api/admin/search?query='.$query)->assertOk();
        $count = count(DB::getQueryLog());
        DB::disableQueryLog();

        return [$response->json('equipment'), $count];
    }

    public function test_query_count_does_not_grow_with_results(): void
    {
        $this->makeHits('winch', 2);
        $this->makeHits('hoist', 12);

        $this->search('nothingmatches'); // warm up per-request setup
        [$small, $smallQueries] = $this->search('zqwinch');
        [$big, $bigQueries] = $this->search('zqhoist');

        $this->assertCount(2, $small);
        $this->assertCount(12, $big);
        foreach ($big as $row) {
            $this->assertStringStartsWith('Holder hoist', $row['client']['company_name']);
        }
        $this->assertLessThanOrEqual($smallQueries + 2, $bigQueries, "2 hits: $smallQueries queries, 12 hits: $bigQueries");
    }
}
