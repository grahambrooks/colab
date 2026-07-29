<?php

namespace App\New;

use App\New\Thing;

class Service
{
    public function run($a)
    {
        return Thing::newRun($a, 2);
    }
}
