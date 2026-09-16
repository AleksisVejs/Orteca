<?php

namespace Tests\Feature;

use App\Models\Admin;
use App\Models\Client;
use App\Models\Equipment;
use App\Models\Technician;
use App\Models\User;
use Illuminate\Foundation\Testing\RefreshDatabase;
use Illuminate\Support\Facades\DB;
use Illuminate\Support\Facades\Hash;
use Tests\TestCase;

class BenchToughDeltaSyncTest extends TestCase
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
            'company_name' => 'Sync Co',
            'company_registration_number' => '40000000004',
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

    private function makeClient(string $name): Client
    {
        $user = $this->makeUser('client');

        return Client::create(['user_id' => $user->id, 'admin_id' => $this->admin->id, 'company_name' => $name]);
    }

    private function makeTechnician(): User
    {
        $user = $this->makeUser('technician');
        Technician::create([
            'user_id' => $user->id,
            'admin_id' => $this->admin->id,
            'email' => $user->email,
            'name' => $user->name,
            'is_active' => true,
            'invitation_accepted' => true,
            'invitation_accepted_at' => now(),
        ]);

        return $user;
    }

    private function equipment(string $name, ?\DateTimeInterface $updated = null, ?\DateTimeInterface $deleted = null): Equipment
    {
        $item = Equipment::create(['name' => $name, 'admin_id' => $this->admin->id]);
        $row = [];
        if ($updated) {
            $row['updated_at'] = $updated;
        }
        if ($deleted) {
            $row['deleted_at'] = $deleted;
            $row['updated_at'] = $deleted;
        }
        if ($row) {
            DB::table($item->getTable())->where('id', $item->id)->update($row);
        }

        return $item;
    }

    private function client(string $name, ?\DateTimeInterface $updated = null, ?\DateTimeInterface $deleted = null): Client
    {
        $client = $this->makeClient($name);
        $row = [];
        if ($updated) {
            $row['updated_at'] = $updated;
        }
        if ($deleted) {
            $row['deleted_at'] = $deleted;
            $row['updated_at'] = $deleted;
        }
        if ($row) {
            DB::table($client->getTable())->where('user_id', $client->user_id)->update($row);
        }

        return $client;
    }

    private function since(): string
    {
        return rawurlencode(now()->subDay()->toIso8601String());
    }

    public function test_without_since_is_a_full_snapshot(): void
    {
        $old = $this->equipment('Old crane', now()->subDays(2));
        $client = $this->client('Old client', now()->subDays(2));

        $response = $this->actingAs($this->adminUser)->getJson('/api/admin/sync/bootstrap')->assertOk();

        $this->assertTrue($response->json('full'));
        $this->assertContains($old->id, collect($response->json('equipment'))->pluck('id')->all());
        $this->assertContains($client->user_id, collect($response->json('clients'))->pluck('user_id')->all());
        $this->assertNotEmpty($response->json('synced_at'));
    }

    public function test_admin_delta_returns_only_changes_and_deletions(): void
    {
        $this->equipment('Old crane', now()->subDays(2));
        $new = $this->equipment('New crane');
        $gone = $this->equipment('Gone crane', null, now());
        $goneLongAgo = $this->equipment('Ancient crane', null, now()->subDays(3));
        $this->client('Old client', now()->subDays(2));
        $newClient = $this->client('New client');
        $goneClient = $this->client('Gone client', null, now());

        $response = $this->actingAs($this->adminUser)
            ->getJson('/api/admin/sync/bootstrap?since='.$this->since())
            ->assertOk();

        $this->assertFalse($response->json('full'));
        $this->assertEquals([$new->id], collect($response->json('equipment'))->pluck('id')->all());
        $this->assertEquals([$newClient->user_id], collect($response->json('clients'))->pluck('user_id')->all());
        $this->assertContains($gone->id, $response->json('deleted_equipment_ids'));
        $this->assertNotContains($goneLongAgo->id, $response->json('deleted_equipment_ids'));
        $this->assertContains($goneClient->user_id, $response->json('deleted_client_ids'));
        $this->assertNotEmpty($response->json('synced_at'));
    }

    public function test_technician_delta_returns_only_changes(): void
    {
        $tech = $this->makeTechnician();
        $this->equipment('Old crane', now()->subDays(2));
        $new = $this->equipment('New crane');
        $gone = $this->equipment('Gone crane', null, now());

        $response = $this->actingAs($tech)
            ->getJson('/api/technician/sync/bootstrap?since='.$this->since())
            ->assertOk();

        $this->assertFalse($response->json('full'));
        $this->assertEquals([$new->id], collect($response->json('equipment'))->pluck('id')->all());
        $this->assertContains($gone->id, $response->json('deleted_equipment_ids'));

        $full = $this->actingAs($tech)->getJson('/api/technician/sync/bootstrap')->assertOk();
        $this->assertTrue($full->json('full'));
        $this->assertCount(2, $full->json('equipment'));
    }

    public function test_invalid_since_is_rejected(): void
    {
        $this->actingAs($this->adminUser)->getJson('/api/admin/sync/bootstrap?since=not-a-date')->assertStatus(422);
        $this->actingAs($this->makeTechnician())->getJson('/api/technician/sync/bootstrap?since=not-a-date')->assertStatus(422);
    }
}
