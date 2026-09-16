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

/** Added after the runs: the removed per-client query was also the tenant check. */
class BenchMediumTenantTest extends TestCase
{
    use RefreshDatabase;

    private function makeUser(string $type): User
    {
        return User::factory()->create([
            'type' => $type,
            'password' => Hash::make('Password1!'),
            'email_verified_at' => now(),
            'is_active' => true,
        ]);
    }

    private function makeAdmin(string $name, string $reg): array
    {
        $user = $this->makeUser('admin');
        $admin = Admin::create([
            'user_id' => $user->id,
            'company_name' => $name,
            'company_registration_number' => $reg,
            'verification_status' => 'verified',
        ]);
        $this->entitle($admin);

        return [$user, $admin];
    }

    public function test_another_tenants_client_is_not_listed(): void
    {
        [$adminUser, $admin] = $this->makeAdmin('Own Co', '40000000005');
        [, $foreign] = $this->makeAdmin('Other Co', '40000000006');

        $ownUser = $this->makeUser('client');
        Client::create(['user_id' => $ownUser->id, 'admin_id' => $admin->id, 'company_name' => 'Own holder']);
        $foreignUser = $this->makeUser('client');
        Client::create(['user_id' => $foreignUser->id, 'admin_id' => $foreign->id, 'company_name' => 'Foreign holder']);

        $equipment = Equipment::create(['name' => 'Zqtenant crane', 'admin_id' => $admin->id]);
        DB::table('client_equipment')->insert([
            ['equipment_id' => $equipment->id, 'client_id' => $ownUser->id],
            ['equipment_id' => $equipment->id, 'client_id' => $foreignUser->id],
        ]);

        $results = $this->actingAs($adminUser)->getJson('/api/admin/search?query=zqtenant')->assertOk()->json('equipment');

        $listed = collect($results)->pluck('client.user_id')->all();
        $this->assertContains($ownUser->id, $listed);
        // Only the result's client summary: equipment.all_clients was unscoped before any change.
        $this->assertNotContains($foreignUser->id, $listed, 'a client of another company was listed');
    }
}
