<?php

namespace App\Old;

use App\Old\Thing;

class Service
{
    public function run($a)
    {
        return Thing::oldRun($a, 2);
    }
}
